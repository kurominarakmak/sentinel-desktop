use sentinel_provider_api::{
    agent::{AgentToolExecutor, ToolExecutionResult},
    ProviderError, ProviderFuture, ToolCall, ToolDefinition,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_LIST_ENTRIES: usize = 500;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct OwnedWorktreeTools {
    root: PathBuf,
}

impl OwnedWorktreeTools {
    pub(crate) fn new(worktree: &Path, primary: &Path) -> Result<Self, ProviderError> {
        let root = worktree
            .canonicalize()
            .map_err(|_| ProviderError::Unavailable("owned worktree is unavailable".into()))?;
        let primary = primary
            .canonicalize()
            .map_err(|_| ProviderError::Unavailable("primary checkout is unavailable".into()))?;
        if root == primary || !root.is_dir() {
            return Err(ProviderError::Unavailable(
                "owned worktree identity could not be proven".into(),
            ));
        }
        Ok(Self { root })
    }

    fn execute_bounded(&self, call: &ToolCall) -> ToolExecutionResult {
        let result = match call.name.as_str() {
            "list_files" => serde_json::from_value::<PathInput>(call.arguments.clone())
                .map_err(|_| "invalid list_files arguments")
                .and_then(|input| self.list_files(&input.path)),
            "read_file" => serde_json::from_value::<RequiredPathInput>(call.arguments.clone())
                .map_err(|_| "invalid read_file arguments")
                .and_then(|input| self.read_file(&input.path)),
            "write_file" => serde_json::from_value::<WriteInput>(call.arguments.clone())
                .map_err(|_| "invalid write_file arguments")
                .and_then(|input| self.write_file(&input.path, &input.content)),
            "replace_text" => serde_json::from_value::<ReplaceInput>(call.arguments.clone())
                .map_err(|_| "invalid replace_text arguments")
                .and_then(|input| self.replace_text(&input.path, &input.old_text, &input.new_text)),
            "delete_file" => serde_json::from_value::<RequiredPathInput>(call.arguments.clone())
                .map_err(|_| "invalid delete_file arguments")
                .and_then(|input| self.delete_file(&input.path)),
            _ => Err("unsupported tool"),
        };
        ToolExecutionResult {
            content: match result {
                Ok(value) => json!({"ok":true,"result":value}).to_string(),
                Err(error) => json!({"ok":false,"error":error}).to_string(),
            },
        }
    }

    fn list_files(&self, relative: &str) -> Result<serde_json::Value, &'static str> {
        let directory = if relative.is_empty() {
            self.root.clone()
        } else {
            self.existing_path(relative, true)?
        };
        if !directory.is_dir() {
            return Err("list path is not a directory");
        }
        let mut pending = vec![directory];
        let mut files = Vec::new();
        while let Some(directory) = pending.pop() {
            let mut entries = fs::read_dir(&directory)
                .map_err(|_| "directory could not be read")?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "directory could not be read")?;
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                if files.len() >= MAX_LIST_ENTRIES {
                    return Ok(json!({"files":files,"truncated":true}));
                }
                let path = entry.path();
                let relative = path
                    .strip_prefix(&self.root)
                    .map_err(|_| "path identity changed")?;
                if relative.components().next().is_some_and(|component| {
                    matches!(component, Component::Normal(value) if value == OsStr::new(".git"))
                }) {
                    continue;
                }
                let metadata =
                    fs::symlink_metadata(&path).map_err(|_| "file metadata is unavailable")?;
                if metadata.file_type().is_symlink() {
                    files.push(json!({"path":relative.to_string_lossy(),"kind":"symlink"}));
                } else if metadata.is_dir() {
                    pending.push(path);
                } else if metadata.is_file() {
                    files.push(json!({"path":relative.to_string_lossy(),"kind":"file","bytes":metadata.len()}));
                }
            }
        }
        Ok(json!({"files":files,"truncated":false}))
    }

    fn read_file(&self, relative: &str) -> Result<serde_json::Value, &'static str> {
        let path = self.existing_path(relative, false)?;
        let metadata = fs::metadata(&path).map_err(|_| "file metadata is unavailable")?;
        if !metadata.is_file() || metadata.len() as usize > MAX_READ_BYTES {
            return Err("file is not a bounded regular file");
        }
        let bytes = fs::read(path).map_err(|_| "file could not be read")?;
        let content = String::from_utf8(bytes).map_err(|_| "file is not UTF-8 text")?;
        Ok(json!({"content":content,"bytes":content.len()}))
    }

    fn write_file(&self, relative: &str, content: &str) -> Result<serde_json::Value, &'static str> {
        if content.len() > MAX_FILE_BYTES || content.contains('\0') {
            return Err("file content exceeds the bounded text limit");
        }
        let path = self.writable_path(relative)?;
        let parent = path.parent().ok_or("file parent is unavailable")?;
        self.ensure_directories(parent)?;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err("destination is not a regular file");
            }
        }
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(".sentinel-write-{}-{sequence}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "temporary file could not be created")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "temporary file permissions could not be secured")?;
        }
        if file.write_all(content.as_bytes()).is_err() || file.sync_all().is_err() {
            let _ = fs::remove_file(&temporary);
            return Err("file content could not be persisted");
        }
        drop(file);
        if fs::rename(&temporary, &path).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err("file replacement failed");
        }
        Ok(json!({"path":relative,"bytes":content.len()}))
    }

    fn replace_text(
        &self,
        relative: &str,
        old_text: &str,
        new_text: &str,
    ) -> Result<serde_json::Value, &'static str> {
        if old_text.is_empty() {
            return Err("old_text must not be empty");
        }
        let path = self.existing_path(relative, false)?;
        let bytes = fs::read(&path).map_err(|_| "file could not be read")?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err("file exceeds the bounded text limit");
        }
        let content = String::from_utf8(bytes).map_err(|_| "file is not UTF-8 text")?;
        if content.match_indices(old_text).count() != 1 {
            return Err("old_text must match exactly once");
        }
        let replaced = content.replacen(old_text, new_text, 1);
        self.write_file(relative, &replaced)
    }

    fn delete_file(&self, relative: &str) -> Result<serde_json::Value, &'static str> {
        let path = self.existing_path(relative, false)?;
        if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
            return Err("delete target is not a regular file");
        }
        fs::remove_file(path).map_err(|_| "file could not be deleted")?;
        Ok(json!({"path":relative}))
    }

    fn existing_path(&self, relative: &str, directory: bool) -> Result<PathBuf, &'static str> {
        let path = self.lexical_path(relative)?;
        self.reject_symlink_components(&path)?;
        let canonical = path.canonicalize().map_err(|_| "path does not exist")?;
        if !canonical.starts_with(&self.root)
            || (directory && !canonical.is_dir())
            || (!directory && !canonical.is_file())
        {
            return Err("path is outside the owned task worktree");
        }
        Ok(canonical)
    }

    fn writable_path(&self, relative: &str) -> Result<PathBuf, &'static str> {
        let path = self.lexical_path(relative)?;
        self.reject_symlink_components(&path)?;
        Ok(path)
    }

    fn lexical_path(&self, relative: &str) -> Result<PathBuf, &'static str> {
        if relative.is_empty()
            || relative.len() > 4096
            || relative.contains('\0')
            || relative.contains('\\')
        {
            return Err("path is invalid");
        }
        let path = Path::new(relative);
        if path.is_absolute()
            || path.components().any(|component| {
                !matches!(component, Component::Normal(_))
                    || matches!(component, Component::Normal(value) if value == OsStr::new(".git"))
            })
        {
            return Err("path must be a repository-relative non-Git path");
        }
        Ok(self.root.join(path))
    }

    fn reject_symlink_components(&self, path: &Path) -> Result<(), &'static str> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| "path is outside the owned task worktree")?;
        let mut current = self.root.clone();
        for component in relative.components() {
            current.push(component.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err("symbolic-link paths are not writable")
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(_) => return Err("path metadata is unavailable"),
            }
        }
        Ok(())
    }

    fn ensure_directories(&self, parent: &Path) -> Result<(), &'static str> {
        let relative = parent
            .strip_prefix(&self.root)
            .map_err(|_| "directory is outside the owned task worktree")?;
        let mut current = self.root.clone();
        for component in relative.components() {
            current.push(component.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err("directory path is not a real directory")
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&current).map_err(|_| "directory could not be created")?;
                }
                Err(_) => return Err("directory metadata is unavailable"),
            }
        }
        Ok(())
    }
}

impl AgentToolExecutor for OwnedWorktreeTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            tool(
                "list_files",
                "List bounded files below a worktree-relative directory.",
                json!({"type":"object","properties":{"path":{"type":"string"}},"additionalProperties":false}),
            ),
            tool(
                "read_file",
                "Read a bounded UTF-8 file from the owned task worktree.",
                path_schema(),
            ),
            tool(
                "write_file",
                "Atomically write a bounded UTF-8 file inside the owned task worktree.",
                json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
            ),
            tool(
                "replace_text",
                "Replace text that occurs exactly once in a worktree file.",
                json!({"type":"object","properties":{"path":{"type":"string"},"old_text":{"type":"string"},"new_text":{"type":"string"}},"required":["path","old_text","new_text"],"additionalProperties":false}),
            ),
            tool(
                "delete_file",
                "Delete one regular file inside the owned task worktree.",
                path_schema(),
            ),
        ]
    }

    fn execute<'a>(&'a self, call: &'a ToolCall) -> ProviderFuture<'a, ToolExecutionResult> {
        Box::pin(async move { Ok(self.execute_bounded(call)) })
    }
}

fn tool(name: &str, description: &str, parameters: serde_json::Value) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        parameters,
        strict: Some(true),
    }
}

fn path_schema() -> serde_json::Value {
    json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false})
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathInput {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequiredPathInput {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplaceInput {
    path: String,
    old_text: String,
    new_text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_tools_write_replace_read_and_delete_only_inside_worktree() {
        let primary = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let tools = OwnedWorktreeTools::new(worktree.path(), primary.path()).unwrap();
        let write = tools.execute_bounded(&ToolCall {
            id: "1".into(),
            name: "write_file".into(),
            arguments: json!({"path":"src/lib.rs","content":"one"}),
        });
        assert!(write.content.contains("\"ok\":true"));
        let replace = tools.execute_bounded(&ToolCall {
            id: "2".into(),
            name: "replace_text".into(),
            arguments: json!({"path":"src/lib.rs","old_text":"one","new_text":"two"}),
        });
        assert!(replace.content.contains("\"ok\":true"));
        assert_eq!(
            fs::read_to_string(worktree.path().join("src/lib.rs")).unwrap(),
            "two"
        );
        let delete = tools.execute_bounded(&ToolCall {
            id: "3".into(),
            name: "delete_file".into(),
            arguments: json!({"path":"src/lib.rs"}),
        });
        assert!(delete.content.contains("\"ok\":true"));
    }

    #[cfg(unix)]
    #[test]
    fn owned_tools_reject_escape_git_and_symlink_paths() {
        use std::os::unix::fs::symlink;
        let primary = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), worktree.path().join("escape")).unwrap();
        let tools = OwnedWorktreeTools::new(worktree.path(), primary.path()).unwrap();
        for path in ["../outside", ".git/config", "/tmp/outside", "escape/file"] {
            let result = tools.execute_bounded(&ToolCall {
                id: "escape".into(),
                name: "write_file".into(),
                arguments: json!({"path":path,"content":"bad"}),
            });
            assert!(result.content.contains("\"ok\":false"), "{path}");
        }
        assert!(!outside.path().join("file").exists());
    }
}
