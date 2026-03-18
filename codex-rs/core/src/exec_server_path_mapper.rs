use codex_utils_absolute_path::AbsolutePathBuf;

#[derive(Clone, Debug)]
pub(crate) struct RemoteWorkspacePathMapper {
    local_root: AbsolutePathBuf,
    remote_root: AbsolutePathBuf,
}

impl RemoteWorkspacePathMapper {
    pub(crate) fn new(local_root: AbsolutePathBuf, remote_root: AbsolutePathBuf) -> Self {
        Self {
            local_root,
            remote_root,
        }
    }

    pub(crate) fn map_path(&self, path: &AbsolutePathBuf) -> AbsolutePathBuf {
        let Ok(relative) = path.as_path().strip_prefix(self.local_root.as_path()) else {
            return path.clone();
        };
        AbsolutePathBuf::try_from(self.remote_root.as_path().join(relative))
            .expect("workspace remap should preserve an absolute path")
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteWorkspacePathMapper;
    use codex_utils_absolute_path::AbsolutePathBuf;

    #[test]
    fn remaps_path_inside_workspace_root() {
        let mapper = RemoteWorkspacePathMapper::new(
            AbsolutePathBuf::try_from("/Users/starr/code/codex").unwrap(),
            AbsolutePathBuf::try_from("/home/dev-user/codex").unwrap(),
        );
        let path =
            AbsolutePathBuf::try_from("/Users/starr/code/codex/codex-rs/core/src/lib.rs").unwrap();
        assert_eq!(
            mapper.map_path(&path),
            AbsolutePathBuf::try_from("/home/dev-user/codex/codex-rs/core/src/lib.rs").unwrap()
        );
    }

    #[test]
    fn leaves_path_outside_workspace_root_unchanged() {
        let mapper = RemoteWorkspacePathMapper::new(
            AbsolutePathBuf::try_from("/Users/starr/code/codex").unwrap(),
            AbsolutePathBuf::try_from("/home/dev-user/codex").unwrap(),
        );
        let path = AbsolutePathBuf::try_from("/tmp/outside.txt").unwrap();
        assert_eq!(mapper.map_path(&path), path);
    }
}
