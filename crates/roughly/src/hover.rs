use {crate::config::ExperimentalFeatures, typing::HoverInfo};

pub fn markdown(hover_info: &HoverInfo, experimental_features: ExperimentalFeatures) -> String {
    let mut sections = hover_info
        .sections
        .iter()
        .map(|section| {
            format!(
                "### {}\n\n{}",
                section.phase.title(),
                fenced_block("text", &section.value)
            )
        })
        .collect::<Vec<_>>();

    if experimental_features.debug {
        sections.push(format!(
            "### Debug\n\n- range: {}:{} to {}:{}",
            hover_info.range.start.line_index + 1,
            hover_info.range.start.character_index + 1,
            hover_info.range.end.line_index + 1,
            hover_info.range.end.character_index + 1,
        ));
    }

    sections.join("\n\n---\n\n")
}

fn fenced_block(language: &str, contents: &str) -> String {
    format!("```{language}\n{contents}\n```")
}
