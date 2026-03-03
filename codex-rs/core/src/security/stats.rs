#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SecurityStats {
    pub(crate) total: usize,
    pub(crate) allowed: usize,
    pub(crate) denied: usize,
}
