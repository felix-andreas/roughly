use {
    crate::config::ExperimentalFeatures,
    analysis::{HoverInfo, render_hover_markdown},
};

pub fn markdown(hover_info: &HoverInfo, experimental_features: ExperimentalFeatures) -> String {
    render_hover_markdown(hover_info, experimental_features.debug)
}
