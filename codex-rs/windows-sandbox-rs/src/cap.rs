use anyhow::Context;
use anyhow::Result;
use rand::rngs::SmallRng;
use rand::RngCore;
use rand::SeedableRng;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use crate::path_normalization::canonical_path_key;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CapSids {
    pub workspace: String,
    pub readonly: String,
    /// Path-scoped capability SIDs keyed by canonicalized writable-root strings.
    ///
    /// This is used to isolate writable roots from one another so stale ACL grants on
    /// one workspace do not automatically authorize later sessions in unrelated roots.
    ///
    /// The serialized field name is kept for backwards compatibility with existing
    /// `cap_sid` files on disk.
    #[serde(default)]
    pub workspace_by_cwd: HashMap<String, String>,
}

pub fn cap_sid_file(codex_home: &Path) -> PathBuf {
    codex_home.join("cap_sid")
}

fn make_random_cap_sid_string() -> String {
    let mut rng = SmallRng::from_entropy();
    let a = rng.next_u32();
    let b = rng.next_u32();
    let c = rng.next_u32();
    let d = rng.next_u32();
    format!("S-1-5-21-{}-{}-{}-{}", a, b, c, d)
}

fn persist_caps(path: &Path, caps: &CapSids) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create cap sid dir {}", dir.display()))?;
    }
    let json = serde_json::to_string(caps)?;
    fs::write(path, json).with_context(|| format!("write cap sid file {}", path.display()))?;
    Ok(())
}

pub fn load_or_create_cap_sids(codex_home: &Path) -> Result<CapSids> {
    let path = cap_sid_file(codex_home);
    if path.exists() {
        let txt = fs::read_to_string(&path)
            .with_context(|| format!("read cap sid file {}", path.display()))?;
        let t = txt.trim();
        if t.starts_with('{') && t.ends_with('}') {
            if let Ok(obj) = serde_json::from_str::<CapSids>(t) {
                return Ok(obj);
            }
        } else if !t.is_empty() {
            let caps = CapSids {
                workspace: t.to_string(),
                readonly: make_random_cap_sid_string(),
                workspace_by_cwd: HashMap::new(),
            };
            persist_caps(&path, &caps)?;
            return Ok(caps);
        }
    }
    let caps = CapSids {
        workspace: make_random_cap_sid_string(),
        readonly: make_random_cap_sid_string(),
        workspace_by_cwd: HashMap::new(),
    };
    persist_caps(&path, &caps)?;
    Ok(caps)
}

/// Returns the workspace-specific capability SID for `cwd`, creating and persisting it if missing.
pub fn workspace_cap_sid_for_cwd(codex_home: &Path, cwd: &Path) -> Result<String> {
    write_cap_sid_for_root(codex_home, cwd)
}

/// Returns the path-scoped capability SID for `root`, creating and persisting it if missing.
pub fn write_cap_sid_for_root(codex_home: &Path, root: &Path) -> Result<String> {
    let path = cap_sid_file(codex_home);
    let mut caps = load_or_create_cap_sids(codex_home)?;
    let key = canonical_path_key(root);
    if let Some(sid) = caps.workspace_by_cwd.get(&key) {
        return Ok(sid.clone());
    }
    let sid = make_random_cap_sid_string();
    caps.workspace_by_cwd.insert(key, sid.clone());
    persist_caps(&path, &caps)?;
    Ok(sid)
}

#[cfg(test)]
mod tests {
    use super::load_or_create_cap_sids;
    use super::write_cap_sid_for_root;
    use super::workspace_cap_sid_for_cwd;
    use pretty_assertions::assert_eq;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn equivalent_cwd_spellings_share_workspace_sid_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let codex_home = temp.path().join("codex-home");
        std::fs::create_dir_all(&codex_home).expect("create codex home");

        let workspace = temp.path().join("WorkspaceRoot");
        std::fs::create_dir_all(&workspace).expect("create workspace root");

        let canonical = dunce::canonicalize(&workspace).expect("canonical workspace root");
        let alt_spelling = PathBuf::from(canonical.to_string_lossy().replace('\\', "/").to_ascii_uppercase());

        let first_sid =
            workspace_cap_sid_for_cwd(&codex_home, canonical.as_path()).expect("first sid");
        let second_sid =
            workspace_cap_sid_for_cwd(&codex_home, alt_spelling.as_path()).expect("second sid");

        assert_eq!(first_sid, second_sid);

        let caps = load_or_create_cap_sids(&codex_home).expect("load caps");
        assert_eq!(caps.workspace_by_cwd.len(), 1);
    }

    #[test]
    fn distinct_writable_roots_get_distinct_sids() {
        let temp = tempfile::tempdir().expect("tempdir");
        let codex_home = temp.path().join("codex-home");
        std::fs::create_dir_all(&codex_home).expect("create codex home");

        let workspace_a = temp.path().join("WorkspaceA");
        let workspace_b = temp.path().join("WorkspaceB");
        std::fs::create_dir_all(&workspace_a).expect("create workspace a");
        std::fs::create_dir_all(&workspace_b).expect("create workspace b");

        let sid_a = write_cap_sid_for_root(&codex_home, &workspace_a).expect("sid a");
        let sid_b = write_cap_sid_for_root(&codex_home, &workspace_b).expect("sid b");

        assert_ne!(sid_a, sid_b);

        let caps = load_or_create_cap_sids(&codex_home).expect("load caps");
        let values: HashSet<_> = caps.workspace_by_cwd.values().cloned().collect();
        assert_eq!(values.len(), 2);
    }
}
