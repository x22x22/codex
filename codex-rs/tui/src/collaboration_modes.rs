use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::config_types::ModeKind;

fn filtered_presets(collaboration_modes: &[CollaborationModeMask]) -> Vec<CollaborationModeMask> {
    collaboration_modes
        .iter()
        .filter(|&mask| mask.mode.is_some_and(ModeKind::is_tui_visible))
        .cloned()
        .collect()
}

pub(crate) fn presets_for_tui(
    collaboration_modes: &[CollaborationModeMask],
) -> Vec<CollaborationModeMask> {
    filtered_presets(collaboration_modes)
}

pub(crate) fn default_mask(
    collaboration_modes: &[CollaborationModeMask],
) -> Option<CollaborationModeMask> {
    let presets = filtered_presets(collaboration_modes);
    presets
        .iter()
        .find(|mask| mask.mode == Some(ModeKind::Default))
        .cloned()
        .or_else(|| presets.into_iter().next())
}

pub(crate) fn mask_for_kind(
    collaboration_modes: &[CollaborationModeMask],
    kind: ModeKind,
) -> Option<CollaborationModeMask> {
    if !kind.is_tui_visible() {
        return None;
    }
    filtered_presets(collaboration_modes)
        .into_iter()
        .find(|mask| mask.mode == Some(kind))
}

/// Cycle to the next collaboration mode preset in list order.
pub(crate) fn next_mask(
    collaboration_modes: &[CollaborationModeMask],
    current: Option<&CollaborationModeMask>,
) -> Option<CollaborationModeMask> {
    let presets = filtered_presets(collaboration_modes);
    if presets.is_empty() {
        return None;
    }
    let current_kind = current.and_then(|mask| mask.mode);
    let next_index = presets
        .iter()
        .position(|mask| mask.mode == current_kind)
        .map_or(0, |idx| (idx + 1) % presets.len());
    presets.get(next_index).cloned()
}

pub(crate) fn default_mode_mask(
    collaboration_modes: &[CollaborationModeMask],
) -> Option<CollaborationModeMask> {
    mask_for_kind(collaboration_modes, ModeKind::Default)
}

pub(crate) fn plan_mask(
    collaboration_modes: &[CollaborationModeMask],
) -> Option<CollaborationModeMask> {
    mask_for_kind(collaboration_modes, ModeKind::Plan)
}
