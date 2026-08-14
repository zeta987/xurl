use std::cmp::Reverse;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::error::{Result, XurlError};
use crate::jsonl;
use crate::model::{ProviderKind, ResolutionMeta, ResolvedThread, WriteRequest, WriteResult};
use crate::provider::{
    Provider, WriteEventSink, append_passthrough_args, append_passthrough_args_excluding,
};

#[derive(Debug, Deserialize)]
struct SessionsIndex {
    #[serde(default)]
    entries: Vec<SessionIndexEntry>,
}

#[derive(Debug, Deserialize)]
struct SessionIndexEntry {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "fullPath")]
    full_path: Option<PathBuf>,
}

/// How much of a transcript's tail to read when looking for its final record.
const TAIL_SCAN_BYTES: u64 = 64 * 1024;

/// How far into a transcript to look for the title record.
///
/// A `custom-title` record is appended when the thread is named, which can
/// happen long after it starts. The metadata line budget is too tight for that
/// — the deepest record seen locally sat at line 59 of the 64 allowed, one turn
/// from being missed — and a missed title is indistinguishable from a thread
/// that never had one. Scanning further is cheap because the expensive JSON
/// parse only runs on lines that mention the record type.
const TITLE_SCAN_LINES: usize = 512;

/// Scans the head of a transcript for the title the user gave the thread.
///
/// Claude writes `custom-title` records early, well inside the caller's line
/// budget, but only when a title was actually set — most threads have none and
/// list without one. See `docs/adr/0001-provider-native-titles-only.md`.
pub(crate) fn read_custom_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().take(TITLE_SCAN_LINES) {
        let Ok(line) = line else { break };
        // Cheap reject first: transcript lines are large, and parsing every one
        // of them to find a record type would dominate the scan.
        if !line.contains("\"custom-title\"") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("custom-title") {
            continue;
        }
        if let Some(title) = value
            .get("customTitle")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return Some(title.to_string());
        }
    }
    None
}

/// Reads the timestamp of the transcript's final record.
///
/// Transcripts are append-only, so the last record carries the thread's real
/// last-active time. Only the tail is read. When the read starts mid-file its
/// first line is dropped: the seek can land inside a multi-byte character, and
/// that leading fragment is never a whole record anyway.
pub(crate) fn read_last_timestamp(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let start = length.saturating_sub(TAIL_SCAN_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;

    let scan_from = if start == 0 {
        0
    } else {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1)
    };
    let text = String::from_utf8_lossy(bytes.get(scan_from..)?);

    text.lines().rev().find_map(|line| {
        let value = serde_json::from_str::<Value>(line).ok()?;
        value
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|stamp| !stamp.is_empty())
            .map(str::to_string)
    })
}

#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    root: PathBuf,
}

impl ClaudeProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn projects_root(&self) -> PathBuf {
        self.root.join("projects")
    }

    fn choose_latest(paths: Vec<PathBuf>) -> Option<(PathBuf, usize)> {
        if paths.is_empty() {
            return None;
        }

        let mut scored = paths
            .into_iter()
            .map(|path| {
                let modified = fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                (path, modified)
            })
            .collect::<Vec<_>>();
        scored.sort_by_key(|(_, modified)| Reverse(*modified));
        let count = scored.len();
        scored.into_iter().next().map(|(path, _)| (path, count))
    }

    fn find_from_sessions_index(projects_root: &Path, session_id: &str) -> Vec<PathBuf> {
        if !projects_root.exists() {
            return Vec::new();
        }

        WalkDir::new(projects_root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.file_name() == "sessions-index.json")
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .filter_map(|content| serde_json::from_str::<SessionsIndex>(&content).ok())
            .flat_map(|index| {
                index.entries.into_iter().filter_map(|entry| {
                    if entry.session_id == session_id {
                        entry.full_path
                    } else {
                        None
                    }
                })
            })
            .filter(|path| path.exists())
            .collect()
    }

    fn find_by_filename(projects_root: &Path, session_id: &str) -> Vec<PathBuf> {
        if !projects_root.exists() {
            return Vec::new();
        }

        let needle = format!("{session_id}.jsonl");
        WalkDir::new(projects_root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name == needle)
            })
            .collect()
    }

    fn file_contains_session_id(path: &Path, session_id: &str) -> bool {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return false,
        };
        let reader = BufReader::new(file);

        for line in reader.lines().take(30).flatten() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&line)
                && value
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id == session_id)
            {
                return true;
            }
        }

        false
    }

    fn find_by_header_scan(projects_root: &Path, session_id: &str) -> Vec<PathBuf> {
        if !projects_root.exists() {
            return Vec::new();
        }

        WalkDir::new(projects_root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext == "jsonl")
            })
            .filter(|path| Self::file_contains_session_id(path, session_id))
            .collect()
    }

    fn make_resolved(
        session_id: &str,
        selected: PathBuf,
        count: usize,
        source: &str,
    ) -> ResolvedThread {
        let mut metadata = ResolutionMeta {
            source: source.to_string(),
            candidate_count: count,
            warnings: Vec::new(),
        };

        if count > 1 {
            metadata.warnings.push(format!(
                "multiple matches found ({count}) for session_id={session_id}; selected latest: {}",
                selected.display()
            ));
        }

        ResolvedThread {
            provider: ProviderKind::Claude,
            session_id: session_id.to_string(),
            path: selected,
            metadata,
        }
    }

    fn claude_bin() -> String {
        std::env::var("XURL_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string())
    }

    fn spawn_claude_command(args: &[String]) -> Result<std::process::Child> {
        let bin = Self::claude_bin();
        let mut command = Command::new(&bin);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                XurlError::CommandNotFound { command: bin }
            } else {
                XurlError::Io {
                    path: PathBuf::from(bin),
                    source,
                }
            }
        })
    }

    fn extract_assistant_text(value: &Value) -> Option<String> {
        let message = value.get("message")?;
        let content = message.get("content")?.as_array()?;
        let text = content
            .iter()
            .filter_map(|item| {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    item.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
        if text.is_empty() { None } else { Some(text) }
    }

    fn run_write(
        &self,
        args: &[String],
        req: &WriteRequest,
        sink: &mut dyn WriteEventSink,
        warnings: Vec<String>,
    ) -> Result<WriteResult> {
        let mut child = Self::spawn_claude_command(args)?;
        let stdout = child.stdout.take().ok_or_else(|| {
            XurlError::WriteProtocol("claude stdout pipe is unavailable".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            XurlError::WriteProtocol("claude stderr pipe is unavailable".to_string())
        })?;
        let stderr_handle = std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut content = String::new();
            let _ = reader.read_to_string(&mut content);
            content
        });

        let mut session_id = req.session_id.clone();
        let mut final_text = None::<String>;
        let stream_path = Path::new("<claude:stdout>");
        let reader = BufReader::new(stdout);
        jsonl::parse_jsonl_reader(stream_path, reader, |_, value| {
            let Some(event_type) = value.get("type").and_then(Value::as_str) else {
                return Ok(());
            };

            match event_type {
                "system" => {
                    if value.get("subtype").and_then(Value::as_str) == Some("init")
                        && let Some(current_session_id) =
                            value.get("session_id").and_then(Value::as_str)
                    {
                        sink.on_session_ready(ProviderKind::Claude, current_session_id)?;
                        session_id = Some(current_session_id.to_string());
                    }
                }
                "assistant" => {
                    if let Some(text) = Self::extract_assistant_text(&value) {
                        sink.on_text_delta(&text)?;
                        final_text = Some(text);
                    }
                    if let Some(current_session_id) =
                        value.get("session_id").and_then(Value::as_str)
                    {
                        session_id = Some(current_session_id.to_string());
                    }
                }
                "result" => {
                    if let Some(current_session_id) =
                        value.get("session_id").and_then(Value::as_str)
                    {
                        session_id = Some(current_session_id.to_string());
                    }
                    if final_text.is_none()
                        && let Some(text) = value.get("result").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        sink.on_text_delta(text)?;
                        final_text = Some(text.to_string());
                    }
                }
                _ => {}
            }
            Ok(())
        })?;

        let status = child.wait().map_err(|source| XurlError::Io {
            path: PathBuf::from(Self::claude_bin()),
            source,
        })?;
        let stderr_content = stderr_handle.join().unwrap_or_default();

        if !status.success() {
            return Err(XurlError::CommandFailed {
                command: format!("{} {}", Self::claude_bin(), args.join(" ")),
                code: status.code(),
                stderr: stderr_content.trim().to_string(),
            });
        }

        let session_id = if let Some(session_id) = session_id {
            session_id
        } else {
            return Err(XurlError::WriteProtocol(
                "missing session id in claude event stream".to_string(),
            ));
        };

        Ok(WriteResult {
            provider: ProviderKind::Claude,
            session_id,
            final_text,
            warnings,
        })
    }
}

impl Provider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn resolve(&self, session_id: &str) -> Result<ResolvedThread> {
        let projects = self.projects_root();

        let index_hits = Self::find_from_sessions_index(&projects, session_id);
        if let Some((selected, count)) = Self::choose_latest(index_hits) {
            return Ok(Self::make_resolved(
                session_id,
                selected,
                count,
                "claude:sessions-index",
            ));
        }

        let filename_hits = Self::find_by_filename(&projects, session_id);
        if let Some((selected, count)) = Self::choose_latest(filename_hits) {
            return Ok(Self::make_resolved(
                session_id,
                selected,
                count,
                "claude:filename",
            ));
        }

        let scanned_hits = Self::find_by_header_scan(&projects, session_id);
        if let Some((selected, count)) = Self::choose_latest(scanned_hits) {
            return Ok(Self::make_resolved(
                session_id,
                selected,
                count,
                "claude:header-scan",
            ));
        }

        Err(XurlError::ThreadNotFound {
            provider: ProviderKind::Claude.to_string(),
            session_id: session_id.to_string(),
            searched_roots: vec![projects],
        })
    }

    fn write(&self, req: &WriteRequest, sink: &mut dyn WriteEventSink) -> Result<WriteResult> {
        let mut warnings = Vec::new();
        let mut args = vec![
            "-p".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];
        if let Some(role) = req.options.role.as_deref() {
            args.push("--agent".to_string());
            args.push(role.to_string());
            let ignored =
                append_passthrough_args_excluding(&mut args, &req.options.params, &["agent"]);
            if !ignored.is_empty() {
                warnings.push(
                    "ignored query parameter `agent` because URI role is already set".to_string(),
                );
            }
        } else {
            append_passthrough_args(&mut args, &req.options.params);
        }
        if let Some(session_id) = req.session_id.as_deref() {
            args.push("--resume".to_string());
            args.push(session_id.to_string());
            args.push(req.prompt.clone());
            self.run_write(&args, req, sink, warnings)
        } else {
            args.push(req.prompt.clone());
            self.run_write(&args, req, sink, warnings)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::provider::Provider;
    use crate::provider::claude::{ClaudeProvider, read_custom_title, read_last_timestamp};

    #[test]
    fn custom_title_is_found_past_the_metadata_budget() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("thread.jsonl");
        let mut transcript = String::new();
        for index in 0..200 {
            transcript.push_str(&format!(
                r#"{{"type":"user","uuid":"u{index}","timestamp":"2026-08-14T10:00:00.000Z"}}"#
            ));
            transcript.push('\n');
        }
        transcript.push_str(
            r#"{"type":"custom-title","customTitle":"Named much later","sessionId":"s"}"#,
        );
        transcript.push('\n');
        fs::write(&path, transcript).expect("write");

        assert_eq!(
            read_custom_title(&path).as_deref(),
            Some("Named much later"),
            "a title set deep into a session is still found"
        );
    }

    #[test]
    fn a_transcript_without_a_title_record_has_no_title() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("thread.jsonl");
        fs::write(&path, "{\"type\":\"user\",\"uuid\":\"u1\"}\n").expect("write");

        assert!(read_custom_title(&path).is_none());
    }

    #[test]
    fn last_timestamp_comes_from_the_final_record() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("thread.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"user","timestamp":"2026-08-14T10:00:00.000Z"}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-08-14T12:34:56.000Z"}"#,
                "\n",
            ),
        )
        .expect("write");

        assert_eq!(
            read_last_timestamp(&path).as_deref(),
            Some("2026-08-14T12:34:56.000Z"),
            "the newest record wins, not the first"
        );
    }

    #[test]
    fn tail_read_survives_a_transcript_larger_than_the_scan_window() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("thread.jsonl");
        let mut transcript = String::new();
        // Each line is padded so the file comfortably exceeds the 64 KiB tail
        // window, forcing the seek to land mid-file where a naive read could
        // split a multi-byte character.
        for index in 0..400 {
            transcript.push_str(&format!(
                r#"{{"type":"user","note":"填充填充填充填充填充填充填充填充填充填充","seq":{index},"timestamp":"2026-08-14T10:00:00.000Z"}}"#
            ));
            transcript.push('\n');
        }
        transcript.push_str(r#"{"type":"assistant","timestamp":"2026-08-14T23:59:59.000Z"}"#);
        transcript.push('\n');
        fs::write(&path, transcript).expect("write");

        assert_eq!(
            read_last_timestamp(&path).as_deref(),
            Some("2026-08-14T23:59:59.000Z")
        );
    }

    #[test]
    fn resolves_from_sessions_index() {
        let temp = tempdir().expect("tempdir");
        let projects = temp.path().join("projects/project-a");
        fs::create_dir_all(&projects).expect("mkdir");
        let thread_file = projects.join("2823d1df-720a-4c31-ac55-ae8ba726721f.jsonl");
        fs::write(&thread_file, "{}\n").expect("write thread");

        let index = projects.join("sessions-index.json");
        fs::write(
            &index,
            format!(
                "{{\"entries\":[{{\"sessionId\":\"2823d1df-720a-4c31-ac55-ae8ba726721f\",\"fullPath\":\"{}\"}}]}}",
                thread_file.display()
            ),
        )
        .expect("write index");

        let provider = ClaudeProvider::new(temp.path());
        let resolved = provider
            .resolve("2823d1df-720a-4c31-ac55-ae8ba726721f")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, thread_file);
        assert_eq!(resolved.metadata.source, "claude:sessions-index");
    }

    #[test]
    fn resolves_from_filename_when_index_misses() {
        let temp = tempdir().expect("tempdir");
        let projects = temp.path().join("projects/project-b");
        fs::create_dir_all(&projects).expect("mkdir");

        let thread_file = projects.join("8c06e0f0-2978-48ac-bb42-90d13e3b0470.jsonl");
        fs::write(&thread_file, "{}\n").expect("write thread");

        let provider = ClaudeProvider::new(temp.path());
        let resolved = provider
            .resolve("8c06e0f0-2978-48ac-bb42-90d13e3b0470")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, thread_file);
        assert_eq!(resolved.metadata.source, "claude:filename");
    }

    #[test]
    fn resolves_from_header_scan() {
        let temp = tempdir().expect("tempdir");
        let projects = temp.path().join("projects/project-c");
        fs::create_dir_all(&projects).expect("mkdir");

        let thread_file = projects.join("renamed.jsonl");
        fs::write(
            &thread_file,
            "{\"type\":\"user\",\"sessionId\":\"1bd3c108-41b8-4291-93e8-8a472ab09de8\"}\n",
        )
        .expect("write thread");

        let provider = ClaudeProvider::new(temp.path());
        let resolved = provider
            .resolve("1bd3c108-41b8-4291-93e8-8a472ab09de8")
            .expect("resolve should succeed");
        assert_eq!(resolved.path, thread_file);
        assert_eq!(resolved.metadata.source, "claude:header-scan");
    }
}
