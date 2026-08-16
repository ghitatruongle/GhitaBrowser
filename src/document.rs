//! Pure document preparation shared by GUI and headless callers.

use std::time::Instant;

use crate::css_parser::{self, CssRule};
use crate::layout::{self, LayoutNode};
use crate::parser::{self, Element};
use crate::text_renderer::TextRenderer;
use crate::RenderStats;

/// Fully prepared, bounded representation of one HTML document.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PreparedDocument {
    pub dom: Element,
    pub title: String,
    pub layout: Option<LayoutNode>,
    pub accessibility: crate::accessibility::AccessibilityTree,
    pub runtime: crate::web_runtime::RuntimeReport,
    pub rendered_text: String,
    pub stats: RenderStats,
}

/// Parse, style, lay out and text-render a document without network or UI
/// side effects. Inline style rules are applied after caller-supplied rules.
pub fn prepare_document(
    html: &str,
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
) -> PreparedDocument {
    prepare_document_impl(
        html,
        fallback_title,
        base_rules,
        viewport_width,
        viewport_height,
        true,
    )
}

/// Prepare an inert document for transfer from the isolated renderer worker.
/// The UI process owns the persistent page runtime, so scripts must not run in
/// the disposable worker and then run a second time after transfer.
pub fn prepare_document_static(
    html: &str,
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
) -> PreparedDocument {
    prepare_document_impl(
        html,
        fallback_title,
        base_rules,
        viewport_width,
        viewport_height,
        false,
    )
}

fn prepare_document_impl(
    html: &str,
    fallback_title: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
    viewport_height: u32,
    execute_scripts: bool,
) -> PreparedDocument {
    let total_start = Instant::now();

    let parse_start = Instant::now();
    let mut dom = parser::parse_html(html);
    let parse_time_ms = parse_start.elapsed().as_millis() as u64;

    let runtime = if execute_scripts {
        crate::web_runtime::run_inline_scripts(&mut dom, fallback_title)
    } else {
        crate::web_runtime::RuntimeReport::default()
    };

    let title = dom
        .find_tag("title")
        .or_else(|| dom.find_tag("h1"))
        .map(|element| element.text.trim().to_string())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| fallback_title.to_string());
    let style_start = Instant::now();
    let mut all_rules = base_rules.to_vec();
    for style in dom.find_all_tags("style") {
        if !style.text.trim().is_empty() {
            all_rules.extend(css_parser::parse_css_with_media(
                style.text.trim(),
                viewport_width,
            ));
        }
    }
    let style_time_ms = style_start.elapsed().as_millis() as u64;

    let layout_start = Instant::now();
    // A prepared document enters the renderer through the live DOM, which
    // assigns stable node identities before producing layout, pixels and
    // accessibility output. Later live mutations use the same refresh path.
    let live = crate::live_dom::LiveDocument::from_element(&dom, all_rules, viewport_width);
    let live_render = live.render_state();
    let dom = live_render.dom.clone();
    let accessibility = live_render.accessibility.clone();
    let layout = live_render.layout.clone();
    let layout_time_ms = layout_start.elapsed().as_millis() as u64;

    let render_start = Instant::now();
    let rendered_text = layout
        .as_ref()
        .map(|root| TextRenderer::new(viewport_width, viewport_height).render_to_text(root))
        .unwrap_or_else(|| "[Empty page]".to_string());
    let render_time_ms = render_start.elapsed().as_millis() as u64;

    let dom_nodes = count_dom_nodes(&dom);
    let layout_nodes = layout.as_ref().map(layout::count_layout_nodes).unwrap_or(0);

    PreparedDocument {
        dom,
        title,
        layout,
        accessibility,
        runtime,
        rendered_text,
        stats: RenderStats {
            parse_time_ms,
            style_time_ms,
            layout_time_ms,
            render_time_ms,
            total_time_ms: total_start.elapsed().as_millis() as u64,
            dom_nodes,
            layout_nodes,
        },
    }
}

/// Build a mutable, identity-stable document for callers that need to apply
/// DOM mutations or dispatch events after the initial document load.
pub fn prepare_live_document(
    html: &str,
    base_url: &str,
    base_rules: &[CssRule],
    viewport_width: u32,
) -> crate::live_dom::LiveDocument {
    let mut dom = parser::parse_html(html);
    let _runtime = crate::web_runtime::run_inline_scripts(&mut dom, base_url);
    let mut rules = base_rules.to_vec();
    for style in dom.find_all_tags("style") {
        if !style.text.trim().is_empty() {
            rules.extend(css_parser::parse_css_with_media(
                style.text.trim(),
                viewport_width,
            ));
        }
    }
    crate::live_dom::LiveDocument::from_element(&dom, rules, viewport_width)
}

fn count_dom_nodes(element: &Element) -> usize {
    1 + element.children.iter().map(count_dom_nodes).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepares_title_inline_style_and_stats() {
        let prepared = prepare_document(
            "<html><head><title>Doc</title><style>p{color:red}</style></head><body><p>x</p></body></html>",
            "fallback",
            &[],
            800,
            600,
        );
        assert_eq!(prepared.title, "Doc");
        assert!(prepared.layout.is_some());
        assert!(prepared.rendered_text.contains('x'));
        assert!(prepared.stats.dom_nodes >= 2);
        assert!(prepared.stats.layout_nodes >= 1);
    }

    #[test]
    fn uses_fallback_title_for_untitled_document() {
        let prepared = prepare_document("<p>hello</p>", "https://example.com", &[], 800, 600);
        assert_eq!(prepared.title, "https://example.com");
    }

    #[test]
    fn static_preparation_defers_scripts_to_the_persistent_embedder_runtime() {
        let prepared = prepare_document_static(
            "<p id='value'>before</p><script>document.getElementById('value').textContent='after'</script>",
            "https://example.test/",
            &[],
            800,
            600,
        );
        assert_eq!(prepared.runtime.scripts_executed, 0);
        assert!(prepared.rendered_text.contains("before"));
        assert!(!prepared.rendered_text.contains("after"));
    }

    #[test]
    fn live_preparation_preserves_identity_and_refreshes_pixels() {
        let mut live = prepare_live_document(
            "<p id='message'>before</p>",
            "https://example.test/",
            &[],
            800,
        );
        let node = live.get_element_by_id("message").unwrap();
        let before = live.render_state().revision;
        live.set_text_content(node, "after").unwrap();
        assert_eq!(live.get_element_by_id("message"), Some(node));
        let render = live.refresh();
        assert!(render.revision > before);
        assert!(render
            .display_list
            .items
            .iter()
            .any(|item| matches!(item, crate::paint::DisplayItem::TextRun { content, .. } if content.contains("after"))));
    }
}
