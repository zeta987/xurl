use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{Result, XurlError};
use crate::model::{ProviderKind, ResolutionMeta, ResolvedThread, WriteRequest, WriteResult};
use crate::provider::{Provider, WriteEventSink, append_passthrough_args};

/// `steps.step_type` carrying the user prompt.
const STEP_TYPE_USER_INPUT: i64 = 14;
/// `steps.step_type` carrying a model turn.
const STEP_TYPE_AGENT_RESPONSE: i64 = 15;
/// `steps.step_type` carrying the generated task summary (holds the title).
const STEP_TYPE_TASK_SUMMARY: i64 = 23;

/// Step payload field holding the user-input body.
const FIELD_USER_INPUT_BODY: u64 = 19;
/// Step payload field holding the agent-response body.
const FIELD_AGENT_RESPONSE_BODY: u64 = 20;
/// Step payload field holding the task-summary body.
const FIELD_TASK_SUMMARY_BODY: u64 = 30;

/// Prompt text inside the user-input body.
const FIELD_USER_TEXT: u64 = 2;
/// User-visible response text inside the agent-response body.
///
/// Field 3 of the same body holds private model reasoning and is deliberately
/// skipped so it never reaches rendered output or search text.
const FIELD_AGENT_TEXT: u64 = 1;
/// Conversation title inside the task-summary body.
const FIELD_SUMMARY_TITLE: u64 = 4;

#[derive(Debug, Clone)]
pub struct AgyProvider {
    root: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AgyMaterializedMetadata {
    pub title: Option<String>,
    pub workspace_path: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct AgyMaterialization {
    pub path: PathBuf,
    pub search_text: String,
    pub metadata: AgyMaterializedMetadata,
}

#[derive(Debug, Clone)]
struct AgyMessage {
    id: String,
    role: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct AgyMetadataCache {
    #[serde(default)]
    conversations: BTreeMap<String, AgyConversationEntry>,
}

#[derive(Debug, Deserialize)]
struct AgyConversationEntry {
    #[serde(default)]
    summary: Option<AgyConversationSummary>,
}

#[derive(Debug, Deserialize)]
struct AgyConversationSummary {
    #[serde(rename = "Title", default)]
    title: Option<String>,
    #[serde(rename = "UpdatedAt", default)]
    updated_at: Option<String>,
    #[serde(rename = "WorkspaceURIs", default)]
    workspace_uris: Option<Vec<String>>,
}

impl AgyProvider {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub(crate) fn conversations_root(&self) -> PathBuf {
        self.root.join("conversations")
    }

    fn staging_dir(&self, session_id: &str) -> PathBuf {
        std::env::temp_dir()
            .join("xurl-agy")
            .join(self.root_key())
            .join(format!("{session_id}.staging"))
    }

    fn materialized_path(&self, session_id: &str) -> PathBuf {
        std::env::temp_dir()
            .join("xurl-agy")
            .join(self.root_key())
            .join(format!("{session_id}.jsonl"))
    }

    fn root_key(&self) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.root.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Returns the conversation database for `session_id`, if it exists.
    pub(crate) fn find_store_candidates(&self, session_id: &str) -> Vec<PathBuf> {
        let conversations_root = self.conversations_root();
        if !conversations_root.exists() {
            return Vec::new();
        }

        let Ok(entries) = fs::read_dir(&conversations_root) else {
            return Vec::new();
        };

        entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path.extension().and_then(|ext| ext.to_str()) == Some("db")
                    && path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem.eq_ignore_ascii_case(session_id))
            })
            .collect()
    }

    /// Copies the conversation database next to its write-ahead log into a
    /// private staging directory.
    ///
    /// The newest messages of an active conversation live entirely in the
    /// `-wal` file, so the `-wal` must travel with the `.db`. Staging a copy
    /// (instead of opening the original in place) keeps SQLite from creating
    /// `-shm`/`-wal` side files inside the user's data directory.
    fn stage_store(&self, store_path: &Path, session_id: &str) -> Result<PathBuf> {
        let staging_dir = self.staging_dir(session_id);
        fs::create_dir_all(&staging_dir).map_err(|source| XurlError::Io {
            path: staging_dir.clone(),
            source,
        })?;

        let file_name = store_path
            .file_name()
            .ok_or_else(|| {
                XurlError::InvalidMode(format!(
                    "agy conversation store has no file name: {}",
                    store_path.display()
                ))
            })?
            .to_owned();
        let staged_db = staging_dir.join(&file_name);

        fs::copy(store_path, &staged_db).map_err(|source| XurlError::Io {
            path: store_path.to_path_buf(),
            source,
        })?;

        // The `-shm` file is intentionally not copied: SQLite rebuilds it from
        // the `-wal`, and a stale copy can misrepresent the log contents.
        let wal_source = Self::sidecar_path(store_path, "-wal");
        if wal_source.exists() {
            let wal_target = Self::sidecar_path(&staged_db, "-wal");
            fs::copy(&wal_source, &wal_target).map_err(|source| XurlError::Io {
                path: wal_source,
                source,
            })?;
        } else {
            // Remove a stale log left by an earlier staging run.
            let _ = fs::remove_file(Self::sidecar_path(&staged_db, "-wal"));
        }
        let _ = fs::remove_file(Self::sidecar_path(&staged_db, "-shm"));

        Ok(staged_db)
    }

    fn sidecar_path(db_path: &Path, suffix: &str) -> PathBuf {
        let mut name = db_path.as_os_str().to_os_string();
        name.push(suffix);
        PathBuf::from(name)
    }

    fn open_store(path: &Path) -> Result<Connection> {
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|source| {
            XurlError::Sqlite {
                path: path.to_path_buf(),
                source,
            }
        })
    }

    fn load_metadata_cache(&self, session_id: &str) -> Option<AgyConversationSummary> {
        let cache_path = self.root.join("cache").join("conversation_metadata.json");
        let raw = fs::read_to_string(cache_path).ok()?;
        let cache = serde_json::from_str::<AgyMetadataCache>(&raw).ok()?;
        cache
            .conversations
            .into_iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(session_id))
            .and_then(|(_, entry)| entry.summary)
    }

    /// Reads the conversation database and writes a materialized JSONL view.
    pub(crate) fn materialize_store(
        &self,
        store_path: &Path,
        session_id: &str,
    ) -> Result<AgyMaterialization> {
        let staged = self.stage_store(store_path, session_id)?;
        let conn = Self::open_store(&staged)?;

        let mut stmt = conn
            .prepare("SELECT idx, step_type, step_payload FROM steps ORDER BY idx")
            .map_err(|source| XurlError::Sqlite {
                path: staged.clone(),
                source,
            })?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })
            .map_err(|source| XurlError::Sqlite {
                path: staged.clone(),
                source,
            })?;

        let mut messages = Vec::new();
        let mut title_from_steps = None::<String>;
        for row in rows {
            let (idx, step_type, payload) = row.map_err(|source| XurlError::Sqlite {
                path: staged.clone(),
                source,
            })?;
            let payload = payload.unwrap_or_default();

            match step_type {
                STEP_TYPE_USER_INPUT => {
                    if let Some(text) = field_bytes(&payload, FIELD_USER_INPUT_BODY)
                        .and_then(|body| field_text(body, FIELD_USER_TEXT))
                    {
                        messages.push(AgyMessage {
                            id: format!("step-{idx}"),
                            role: "user".to_string(),
                            text,
                        });
                    }
                }
                STEP_TYPE_AGENT_RESPONSE => {
                    if let Some(text) = field_bytes(&payload, FIELD_AGENT_RESPONSE_BODY)
                        .and_then(|body| field_text(body, FIELD_AGENT_TEXT))
                    {
                        messages.push(AgyMessage {
                            id: format!("step-{idx}"),
                            role: "assistant".to_string(),
                            text,
                        });
                    }
                }
                STEP_TYPE_TASK_SUMMARY if title_from_steps.is_none() => {
                    title_from_steps = field_bytes(&payload, FIELD_TASK_SUMMARY_BODY)
                        .and_then(|body| field_text(body, FIELD_SUMMARY_TITLE));
                }
                // Remaining step types describe tool activity and bookkeeping.
                // The enum is open-ended (one value per tool), so unknown types
                // are skipped instead of being guessed at.
                _ => {}
            }
        }

        drop(stmt);
        drop(conn);

        let cached = self.load_metadata_cache(session_id);
        let metadata = AgyMaterializedMetadata {
            // The cache's `Preview` field is message content, not a title, so it
            // is deliberately not used as a fallback here — see
            // `docs/adr/0001-provider-native-titles-only.md`.
            title: title_from_steps.or_else(|| {
                cached
                    .as_ref()
                    .and_then(|summary| non_empty(summary.title.as_deref()))
            }),
            workspace_path: cached.as_ref().and_then(|summary| {
                summary
                    .workspace_uris
                    .as_ref()
                    .and_then(|uris| uris.iter().find_map(|uri| decode_file_uri_path(uri)))
            }),
            updated_at: cached
                .as_ref()
                .and_then(|summary| non_empty(summary.updated_at.as_deref())),
        };

        let search_text = messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let output = Self::render_jsonl(session_id, &metadata, &messages);

        let materialized_path = self.materialized_path(session_id);
        if let Some(parent) = materialized_path.parent() {
            fs::create_dir_all(parent).map_err(|source| XurlError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&materialized_path, output).map_err(|source| XurlError::Io {
            path: materialized_path.clone(),
            source,
        })?;

        Ok(AgyMaterialization {
            path: materialized_path,
            search_text,
            metadata,
        })
    }

    fn render_jsonl(
        session_id: &str,
        metadata: &AgyMaterializedMetadata,
        messages: &[AgyMessage],
    ) -> String {
        let mut session_metadata = serde_json::Map::new();
        if let Some(title) = &metadata.title {
            session_metadata.insert("title".to_string(), Value::String(title.clone()));
        }
        if let Some(cwd) = &metadata.workspace_path {
            session_metadata.insert("cwd".to_string(), Value::String(cwd.clone()));
        }
        if let Some(updated_at) = &metadata.updated_at {
            session_metadata.insert("updated_at".to_string(), Value::String(updated_at.clone()));
        }

        let mut lines = Vec::with_capacity(messages.len() + 1);
        lines.push(json!({
            "type": "session",
            "sessionId": session_id,
            "metadata": session_metadata,
        }));

        for message in messages {
            lines.push(json!({
                "type": "message",
                "id": message.id,
                "sessionId": session_id,
                "message": { "role": message.role },
                "parts": [{ "type": "text", "text": message.text }],
            }));
        }

        let mut output = String::new();
        for line in lines {
            let encoded = serde_json::to_string(&line).expect("json serialization should succeed");
            output.push_str(&encoded);
            output.push('\n');
        }
        output
    }

    fn agy_bin() -> String {
        std::env::var("XURL_AGY_BIN").unwrap_or_else(|_| "agy".to_string())
    }

    fn spawn_agy_command(args: &[String]) -> Result<std::process::Child> {
        let bin = Self::agy_bin();
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

    fn run_write(
        &self,
        args: &[String],
        req: &WriteRequest,
        sink: &mut dyn WriteEventSink,
        warnings: Vec<String>,
    ) -> Result<WriteResult> {
        let mut child = Self::spawn_agy_command(args)?;
        let stdout = child.stdout.take().ok_or_else(|| {
            XurlError::WriteProtocol("agy stdout pipe is unavailable".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            XurlError::WriteProtocol("agy stderr pipe is unavailable".to_string())
        })?;
        let stderr_handle = std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut content = String::new();
            let _ = reader.read_to_string(&mut content);
            content
        });

        let stream_path = Path::new("<agy:stdout>");
        let mut current_session_id = req.session_id.clone();
        let mut final_text = None::<String>;
        let mut failure = None::<String>;
        let mut saw_json_event = false;
        // `state:"DONE"` repeats the whole text of a step, so emitted text is
        // tracked per step index and only the new suffix is forwarded.
        let mut emitted_by_step = BTreeMap::<i64, String>::new();

        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.map_err(|source| XurlError::Io {
                path: stream_path.to_path_buf(),
                source,
            })?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            saw_json_event = true;

            match value.get("event").and_then(Value::as_str) {
                Some("init") => {
                    if let Some(session_id) = value.get("conversation_id").and_then(Value::as_str)
                        && current_session_id.as_deref() != Some(session_id)
                    {
                        sink.on_session_ready(ProviderKind::Agy, session_id)?;
                        current_session_id = Some(session_id.to_string());
                    }
                }
                Some("step_update") => {
                    let Some(update) = value.get("step_update") else {
                        continue;
                    };
                    if update.get("step_type").and_then(Value::as_str) != Some("agent_response") {
                        continue;
                    }
                    let Some(delta) = update.get("text_delta").and_then(Value::as_str) else {
                        continue;
                    };
                    let step_index = update
                        .get("step_index")
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    let emitted = emitted_by_step.entry(step_index).or_default();
                    if let Some(suffix) = delta.strip_prefix(emitted.as_str()) {
                        if !suffix.is_empty() {
                            sink.on_text_delta(suffix)?;
                        }
                        *emitted = delta.to_string();
                    } else {
                        sink.on_text_delta(delta)?;
                        emitted.push_str(delta);
                    }
                }
                Some("result") => {
                    let Some(result) = value.get("result") else {
                        continue;
                    };
                    if let Some(session_id) = result.get("conversation_id").and_then(Value::as_str)
                        && current_session_id.as_deref() != Some(session_id)
                    {
                        sink.on_session_ready(ProviderKind::Agy, session_id)?;
                        current_session_id = Some(session_id.to_string());
                    }
                    // `response` carries the complete, correctly-encoded answer;
                    // streamed deltas can split multi-byte characters.
                    final_text = result
                        .get("response")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(ToString::to_string);
                    if let Some(status) = result.get("status").and_then(Value::as_str)
                        && !status.eq_ignore_ascii_case("SUCCESS")
                    {
                        failure = Some(status.to_string());
                    }
                }
                _ => {}
            }
        }

        let status = child.wait().map_err(|source| XurlError::Io {
            path: PathBuf::from(Self::agy_bin()),
            source,
        })?;
        let stderr_content = stderr_handle.join().unwrap_or_default();
        if !status.success() {
            return Err(XurlError::CommandFailed {
                command: format!("{} {}", Self::agy_bin(), args.join(" ")),
                code: status.code(),
                stderr: stderr_content.trim().to_string(),
            });
        }

        if !saw_json_event {
            return Err(XurlError::WriteProtocol(format!(
                "agy produced no stream-json events; rerun `{} {}` to inspect its output",
                Self::agy_bin(),
                args.join(" ")
            )));
        }

        if let Some(status) = failure {
            return Err(XurlError::WriteProtocol(format!(
                "agy reported status={status}; inspect the conversation with `xurl agents://agy/{}`",
                current_session_id.as_deref().unwrap_or("<conversation-id>")
            )));
        }

        let session_id = current_session_id.ok_or_else(|| {
            XurlError::WriteProtocol(
                "agy stream-json output did not report a conversation id".to_string(),
            )
        })?;

        Ok(WriteResult {
            provider: ProviderKind::Agy,
            session_id,
            final_text,
            warnings,
        })
    }
}

impl Provider for AgyProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Agy
    }

    fn resolve(&self, session_id: &str) -> Result<ResolvedThread> {
        let candidates = self.find_store_candidates(session_id);
        let count = candidates.len();
        if let Some(selected) = candidates.into_iter().next() {
            let materialized = self.materialize_store(&selected, session_id)?;
            let metadata = ResolutionMeta {
                source: "agy:conversation.db".to_string(),
                candidate_count: count,
                warnings: Vec::new(),
            };

            return Ok(ResolvedThread {
                provider: ProviderKind::Agy,
                session_id: session_id.to_string(),
                path: materialized.path,
                metadata,
            });
        }

        Err(XurlError::ThreadNotFound {
            provider: ProviderKind::Agy.to_string(),
            session_id: session_id.to_string(),
            searched_roots: vec![self.conversations_root()],
        })
    }

    fn write(&self, req: &WriteRequest, sink: &mut dyn WriteEventSink) -> Result<WriteResult> {
        if req.options.role.is_some() {
            return Err(XurlError::InvalidMode(
                "agy does not support role-based write URI; use agents://agy or agents://agy/<conversation-id>".to_string(),
            ));
        }

        let mut args = vec![
            "-p".to_string(),
            req.prompt.clone(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];
        if let Some(session_id) = &req.session_id {
            args.push("--conversation".to_string());
            args.push(session_id.clone());
        }
        append_passthrough_args(&mut args, &req.options.params);

        self.run_write(&args, req, sink, Vec::new())
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToString::to_string)
}

fn read_varint(bytes: &[u8], index: &mut usize) -> Option<u64> {
    let mut shift = 0_u32;
    let mut value = 0_u64;
    while *index < bytes.len() && shift <= 63 {
        let byte = bytes[*index];
        *index += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }

    None
}

/// Returns the payload of the first length-delimited field with `field_number`.
///
/// The walker is schema-agnostic: it decodes protobuf wire types without a
/// generated descriptor, so unknown fields are skipped instead of failing.
fn field_bytes(bytes: &[u8], field_number: u64) -> Option<&[u8]> {
    let mut index = 0;
    while index < bytes.len() {
        let key = read_varint(bytes, &mut index)?;
        let number = key >> 3;
        if number == 0 {
            return None;
        }
        match key & 0x07 {
            0 => {
                read_varint(bytes, &mut index)?;
            }
            1 => {
                index = index.checked_add(8).filter(|end| *end <= bytes.len())?;
            }
            2 => {
                let length = usize::try_from(read_varint(bytes, &mut index)?).ok()?;
                let end = index.checked_add(length)?;
                if end > bytes.len() {
                    return None;
                }
                if number == field_number {
                    return Some(&bytes[index..end]);
                }
                index = end;
            }
            5 => {
                index = index.checked_add(4).filter(|end| *end <= bytes.len())?;
            }
            _ => return None,
        }
    }

    None
}

/// Returns the field payload as trimmed-non-empty UTF-8 text.
fn field_text(bytes: &[u8], field_number: u64) -> Option<String> {
    let payload = field_bytes(bytes, field_number)?;
    let text = std::str::from_utf8(payload).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn decode_file_uri_path(uri: &str) -> Option<String> {
    let path = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))?;
    let mut output = Vec::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hi = decode_hex_nibble(bytes[index + 1])?;
                let lo = decode_hex_nibble(bytes[index + 2])?;
                output.push((hi << 4) | lo);
                index += 3;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(output).ok()
}

fn decode_hex_nibble(ch: u8) -> Option<u8> {
    match ch {
        b'0'..=b'9' => Some(ch - b'0'),
        b'a'..=b'f' => Some(10 + ch - b'a'),
        b'A'..=b'F' => Some(10 + ch - b'A'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AgyProvider, decode_file_uri_path, field_bytes, field_text};
    use crate::error::XurlError;
    use crate::model::ProviderKind;
    use crate::provider::Provider;
    use rusqlite::Connection;
    use std::path::Path;
    use tempfile::TempDir;

    /// Encodes a protobuf varint.
    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = u8::try_from(value & 0x7f).expect("7-bit chunk fits in u8");
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    /// Encodes a length-delimited protobuf field.
    fn length_delimited(field_number: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = varint((field_number << 3) | 2);
        out.extend(varint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    /// Encodes a varint protobuf field.
    fn varint_field(field_number: u64, value: u64) -> Vec<u8> {
        let mut out = varint(field_number << 3);
        out.extend(varint(value));
        out
    }

    /// Builds a synthetic step payload matching the observed agy layout.
    fn step_payload(step_type: u64, body_field: u64, body: &[u8]) -> Vec<u8> {
        let mut out = varint_field(1, step_type);
        out.extend(varint_field(4, 3));
        // Envelope field 5 carries timestamps and ids that the reader skips.
        out.extend(length_delimited(5, &varint_field(3, 4)));
        out.extend(length_delimited(body_field, body));
        out
    }

    fn user_step(step_type: u64, text: &str) -> Vec<u8> {
        step_payload(step_type, 19, &length_delimited(2, text.as_bytes()))
    }

    fn agent_step(step_type: u64, visible: &str, reasoning: &str) -> Vec<u8> {
        let mut body = length_delimited(1, visible.as_bytes());
        body.extend(length_delimited(3, reasoning.as_bytes()));
        step_payload(step_type, 20, &body)
    }

    fn summary_step(title: &str) -> Vec<u8> {
        step_payload(23, 30, &length_delimited(4, title.as_bytes()))
    }

    /// Creates a conversation database with the real agy schema.
    fn build_conversation(root: &Path, session_id: &str, steps: &[(i64, Vec<u8>)]) {
        let conversations = root.join("conversations");
        std::fs::create_dir_all(&conversations).expect("create conversations dir");
        let db_path = conversations.join(format!("{session_id}.db"));
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE `trajectory_meta` (
                `trajectory_id` text, `cascade_id` text,
                `trajectory_type` integer, `source` integer,
                PRIMARY KEY (`trajectory_id`));
             CREATE TABLE `steps` (
                `idx` integer, `step_type` integer NOT NULL DEFAULT 0,
                `status` integer NOT NULL DEFAULT 0,
                `has_subtrajectory` numeric NOT NULL DEFAULT false,
                `metadata` blob, `error_details` blob, `permissions` blob,
                `task_details` blob, `render_info` blob,
                `step_payload` blob, `step_format` integer NOT NULL DEFAULT 0,
                PRIMARY KEY (`idx`));",
        )
        .expect("create schema");
        conn.execute(
            "INSERT INTO trajectory_meta VALUES (?1, ?2, 4, 17)",
            rusqlite::params!["11111111-2222-3333-4444-555555555555", session_id],
        )
        .expect("insert trajectory meta");
        for (idx, (step_type, payload)) in steps.iter().enumerate() {
            conn.execute(
                "INSERT INTO steps (idx, step_type, status, step_payload, step_format)
                 VALUES (?1, ?2, 3, ?3, 0)",
                rusqlite::params![i64::try_from(idx).expect("idx fits"), step_type, payload],
            )
            .expect("insert step");
        }
        conn.close().expect("close db");
    }

    #[test]
    fn resolves_conversation_and_extracts_visible_messages() {
        let temp = TempDir::new().expect("temp dir");
        let session_id = "265b7c4a-eeab-4f2d-84c3-cb7870a3a9a2";
        build_conversation(
            temp.path(),
            session_id,
            &[
                (14, user_step(14, "port the config module")),
                (
                    15,
                    agent_step(15, "Ported the config module.", "hidden reasoning"),
                ),
                (8, vec![0x01]),
            ],
        );

        let provider = AgyProvider::new(temp.path());
        let resolved = provider.resolve(session_id).expect("resolve");
        assert_eq!(resolved.provider, ProviderKind::Agy);
        assert_eq!(resolved.session_id, session_id);
        assert_eq!(resolved.metadata.source, "agy:conversation.db");

        let raw = std::fs::read_to_string(&resolved.path).expect("read materialized");
        assert!(raw.contains("port the config module"));
        assert!(raw.contains("Ported the config module."));
        // Model reasoning must never reach the materialized view.
        assert!(!raw.contains("hidden reasoning"));
    }

    #[test]
    fn uses_task_summary_title_as_thread_name() {
        let temp = TempDir::new().expect("temp dir");
        let session_id = "3f1d1f7e-1111-4222-8333-444444444444";
        build_conversation(
            temp.path(),
            session_id,
            &[
                (14, user_step(14, "review the port")),
                (23, summary_step("Rust Configuration Port Review")),
            ],
        );

        let provider = AgyProvider::new(temp.path());
        let materialized = provider
            .materialize_store(
                &temp
                    .path()
                    .join("conversations")
                    .join(format!("{session_id}.db")),
                session_id,
            )
            .expect("materialize");
        assert_eq!(
            materialized.metadata.title.as_deref(),
            Some("Rust Configuration Port Review")
        );
        assert!(materialized.search_text.contains("review the port"));
    }

    #[test]
    fn missing_conversation_reports_searched_root() {
        let temp = TempDir::new().expect("temp dir");
        let provider = AgyProvider::new(temp.path());
        let err = provider
            .resolve("00000000-0000-0000-0000-000000000000")
            .expect_err("missing conversation must fail");

        // The CLI surfaces `searched_roots` as agent-facing evidence, so the
        // provider must report the directory it actually looked in.
        match err {
            XurlError::ThreadNotFound {
                provider,
                session_id,
                searched_roots,
            } => {
                assert_eq!(provider, "agy");
                assert_eq!(session_id, "00000000-0000-0000-0000-000000000000");
                assert_eq!(searched_roots, vec![temp.path().join("conversations")]);
            }
            other => panic!("expected ThreadNotFound, got: {other:?}"),
        }
    }

    #[test]
    fn reads_newest_rows_from_the_write_ahead_log() {
        let temp = TempDir::new().expect("temp dir");
        let session_id = "1c15ca1c-3077-4166-9089-b0d17082aee7";
        build_conversation(
            temp.path(),
            session_id,
            &[(14, user_step(14, "first prompt"))],
        );

        let db_path = temp
            .path()
            .join("conversations")
            .join(format!("{session_id}.db"));

        // Append a row that stays in the write-ahead log by holding the
        // connection open in WAL mode without checkpointing.
        let conn = Connection::open(&db_path).expect("open db");
        conn.pragma_update(None, "journal_mode", "WAL")
            .expect("enable wal");
        conn.execute(
            "INSERT INTO steps (idx, step_type, status, step_payload, step_format)
             VALUES (99, 15, 3, ?1, 0)",
            rusqlite::params![agent_step(15, "answer that lives in the wal", "reasoning")],
        )
        .expect("insert wal row");

        let provider = AgyProvider::new(temp.path());
        let materialized = provider
            .materialize_store(&db_path, session_id)
            .expect("materialize");
        let raw = std::fs::read_to_string(&materialized.path).expect("read materialized");
        drop(conn);

        assert!(
            raw.contains("answer that lives in the wal"),
            "wal-resident row missing from materialized output: {raw}"
        );
    }

    #[test]
    fn staging_does_not_touch_the_source_directory() {
        let temp = TempDir::new().expect("temp dir");
        let session_id = "a1b2c3d4-0000-4000-8000-000000000000";
        build_conversation(temp.path(), session_id, &[(14, user_step(14, "hello"))]);

        let conversations = temp.path().join("conversations");
        let before = std::fs::read_dir(&conversations)
            .expect("read dir")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name())
            .collect::<std::collections::BTreeSet<_>>();

        let provider = AgyProvider::new(temp.path());
        provider.resolve(session_id).expect("resolve");

        let after = std::fs::read_dir(&conversations)
            .expect("read dir")
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            before, after,
            "reading a conversation must not create files in the agy data directory"
        );
    }

    #[test]
    fn field_lookup_skips_unknown_wire_types() {
        let mut payload = varint_field(1, 15);
        payload.extend(length_delimited(20, b"body"));
        assert_eq!(field_bytes(&payload, 20), Some(b"body".as_slice()));
        assert_eq!(field_bytes(&payload, 21), None);
        assert_eq!(field_text(&payload, 20).as_deref(), Some("body"));
    }

    #[test]
    fn truncated_payload_does_not_panic() {
        let payload = [0xa2_u8, 0x01, 0xff, 0xff];
        assert_eq!(field_bytes(&payload, 20), None);
    }

    #[test]
    fn blank_field_text_is_ignored() {
        let payload = length_delimited(1, b"   ");
        assert_eq!(field_text(&payload, 1), None);
    }

    #[test]
    fn decodes_percent_encoded_file_uri() {
        assert_eq!(
            decode_file_uri_path("file:///D:/Data/My%20Project").as_deref(),
            Some("D:/Data/My Project")
        );
    }
}
