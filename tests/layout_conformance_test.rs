use ghitabrowser::css_parser::parse_css;
use ghitabrowser::layout::{create_layout_tree, perform_layout, DisplayType, LayoutNode};
use ghitabrowser::paint::{build_display_list, calculate_visible_metrics, DisplayItem};
use ghitabrowser::parser::parse_html;

fn find_by_class<'a>(node: &'a LayoutNode, class_name: &str) -> Option<&'a LayoutNode> {
    if node
        .element
        .get_attr("class")
        .map(|c| c.split_whitespace().any(|s| s == class_name))
        .unwrap_or(false)
    {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_by_class(child, class_name))
}

#[test]
fn test_box_model_content_and_border_box() {
    let html = r#"
        <div class="content-box">A</div>
        <div class="border-box">B</div>
        <div class="min-max">C</div>
    "#;
    let css = r#"
        .content-box {
            box-sizing: content-box;
            width: 200px;
            padding: 10px;
            border: 5px solid black;
        }
        .border-box {
            box-sizing: border-box;
            width: 200px;
            padding: 10px;
            border: 5px solid black;
        }
        .min-max {
            width: 500px;
            max-width: 300px;
            min-width: 100px;
        }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let c_box = find_by_class(&layout, "content-box").unwrap();
    assert!(
        (c_box.rect.width - 230.0).abs() < 1.0,
        "content-box outer width should be 200 + 20 + 10 = 230"
    );
    assert!((c_box.rect.content_width() - 200.0).abs() < 1.0);

    let b_box = find_by_class(&layout, "border-box").unwrap();
    assert!(
        (b_box.rect.width - 200.0).abs() < 1.0,
        "border-box outer width should be exactly 200"
    );
    assert!((b_box.rect.content_width() - 170.0).abs() < 1.0);

    let mm_box = find_by_class(&layout, "min-max").unwrap();
    assert!(
        (mm_box.rect.width - 300.0).abs() < 1.0,
        "max-width: 300px must clamp width of 500px"
    );
}

#[test]
fn test_auto_margin_centering_and_offsets() {
    let html = r#"
        <div class="container">
            <div class="centered">Center</div>
            <div class="right-aligned">Right</div>
        </div>
    "#;
    let css = r#"
        .container { width: 800px; }
        .centered { width: 400px; margin: 0 auto; }
        .right-aligned { width: 300px; margin-left: auto; margin-right: 0; }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let centered = find_by_class(&layout, "centered").unwrap();
    assert_eq!(centered.rect.margin_left, 200.0);
    assert_eq!(centered.rect.margin_right, 200.0);
    assert_eq!(centered.rect.x, 200.0);

    let right = find_by_class(&layout, "right-aligned").unwrap();
    assert_eq!(right.rect.margin_left, 500.0);
    assert_eq!(right.rect.x, 500.0);
}

#[test]
fn test_text_wrapping_and_whitespace_modes() {
    let html = r#"
        <p class="nowrap">This is a long sentence that must not wrap onto multiple lines even if it overflows the container width.</p>
        <p class="pre">Line 1
Line 2
Line 3</p>
    "#;
    let css = r#"
        p { width: 150px; font-size: 16px; }
        .nowrap { white-space: nowrap; }
        .pre { white-space: pre; }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let layout = create_layout_tree(&dom, &rules, 800).unwrap();
    let display_list = build_display_list(&layout);

    let nowrap_runs: Vec<&DisplayItem> = display_list
        .items
        .iter()
        .filter(|i| matches!(i, DisplayItem::TextRun { content, .. } if content.contains("sentence that must not wrap")))
        .collect();
    assert_eq!(
        nowrap_runs.len(),
        1,
        "white-space: nowrap must produce exactly 1 line run"
    );

    let pre_runs: Vec<&DisplayItem> = display_list
        .items
        .iter()
        .filter(
            |i| matches!(i, DisplayItem::TextRun { content, .. } if content.starts_with("Line")),
        )
        .collect();
    assert_eq!(
        pre_runs.len(),
        3,
        "white-space: pre must preserve all 3 explicit lines"
    );
}

#[test]
fn test_block_formatting_context_and_margin_collapsing() {
    let html = r#"
        <div class="box1">Box 1</div>
        <div class="box2">Box 2</div>
    "#;
    let css = r#"
        .box1 { margin-bottom: 30px; height: 50px; }
        .box2 { margin-top: 20px; height: 50px; }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let b1 = find_by_class(&layout, "box1").unwrap();
    let b2 = find_by_class(&layout, "box2").unwrap();

    let gap = b2.rect.y - (b1.rect.y + b1.rect.height);
    assert_eq!(
        gap, 30.0,
        "Vertical margins (30px and 20px) must collapse to max(30, 20) = 30px"
    );
}

#[test]
fn test_floats_and_clearance() {
    let html = r#"
        <div class="container">
            <div class="float-box">Float</div>
            <p class="content">Content flowing around float</p>
            <div class="cleared">Cleared below float</div>
        </div>
    "#;
    let css = r#"
        .container { width: 600px; }
        .float-box { float: left; width: 200px; height: 100px; }
        .content { width: 600px; }
        .cleared { clear: both; height: 40px; }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let f = find_by_class(&layout, "float-box").unwrap();
    let c = find_by_class(&layout, "cleared").unwrap();

    assert_eq!(f.rect.x, 0.0);
    assert!(
        c.rect.y >= f.rect.y + f.rect.height,
        "Cleared box must sit at or below the float bottom"
    );
}

#[test]
fn test_positioning_and_containing_blocks() {
    let html = r#"
        <div class="relative-parent">
            <div class="absolute-child">Abs</div>
            <div class="relative-child">Rel</div>
        </div>
    "#;
    let css = r#"
        .relative-parent {
            position: relative;
            left: 50px;
            top: 20px;
            width: 400px;
            height: 300px;
        }
        .absolute-child {
            position: absolute;
            right: 10px;
            top: 15px;
            width: 100px;
            height: 50px;
        }
        .relative-child {
            position: relative;
            left: 30px;
            top: 10px;
            width: 80px;
            height: 40px;
        }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let parent = find_by_class(&layout, "relative-parent").unwrap();
    assert_eq!(parent.rect.x, 50.0);
    assert_eq!(parent.rect.y, 20.0);

    let abs = find_by_class(&layout, "absolute-child").unwrap();
    // Parent x = 50, width = 400. right = 10, width = 100 => 50 + 400 - 10 - 100 = 340
    assert_eq!(abs.rect.x, 340.0);
    assert_eq!(abs.rect.y, 35.0); // 20 + 15 = 35

    let rel = find_by_class(&layout, "relative-child").unwrap();
    assert_eq!(rel.rect.x, parent.rect.x + 30.0);
}

#[test]
fn test_overflow_clipping_and_scroll_extents() {
    let html = r#"
        <div class="clipped-box">
            <div class="overflowing-child">Deep inside</div>
        </div>
    "#;
    let css = r#"
        .clipped-box {
            width: 200px;
            height: 100px;
            overflow: hidden;
        }
        .overflowing-child {
            width: 500px;
            height: 400px;
            margin-top: 300px;
        }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let layout = create_layout_tree(&dom, &rules, 800).unwrap();
    let display_list = build_display_list(&layout);

    let clipped_texts = display_list
        .items
        .iter()
        .filter(|i| matches!(i, DisplayItem::TextRun { content, .. } if content.contains("Deep inside")))
        .count();
    assert_eq!(
        clipped_texts, 0,
        "Overflowing text placed 300px below a 100px overflow:hidden container must be clipped"
    );
}

#[test]
fn test_flexbox_formatting_context() {
    let html = r#"
        <div class="flex-row">
            <div class="item1">One</div>
            <div class="item2">Two</div>
            <div class="item3">Three</div>
        </div>
    "#;
    let css = r#"
        .flex-row {
            display: flex;
            width: 600px;
            gap: 15px;
            justify-content: space-between;
            align-items: center;
        }
        .item1 { flex-grow: 1; height: 40px; }
        .item2 { flex-grow: 2; height: 60px; }
        .item3 { flex-grow: 1; height: 30px; }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let flex = find_by_class(&layout, "flex-row").unwrap();
    assert_eq!(flex.rect.display, DisplayType::Flex);
    assert_eq!(flex.children.len(), 3);

    let item1 = &flex.children[0];
    let item2 = &flex.children[1];
    let item3 = &flex.children[2];

    assert!(
        item2.rect.width > item1.rect.width,
        "flex-grow: 2 must be wider than flex-grow: 1"
    );
    assert!(
        (item1.rect.width - item3.rect.width).abs() < 1.0,
        "Equal flex-grow items must have equal width"
    );
    assert!(item2.rect.x > item1.rect.x + item1.rect.width);
    assert!(item3.rect.x > item2.rect.x + item2.rect.width);
}

#[test]
fn test_grid_formatting_context() {
    let html = r#"
        <div class="grid-container">
            <div class="cell">Cell 1</div>
            <div class="cell">Cell 2</div>
            <div class="cell">Cell 3</div>
            <div class="cell">Cell 4</div>
        </div>
    "#;
    let css = r#"
        .grid-container {
            display: grid;
            grid-template-columns: repeat(2, 1fr);
            gap: 20px;
            width: 420px;
        }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let grid = find_by_class(&layout, "grid-container").unwrap();
    assert_eq!(grid.rect.display, DisplayType::Grid);
    assert_eq!(grid.children.len(), 4);

    let c0 = &grid.children[0];
    let c1 = &grid.children[1];
    let c2 = &grid.children[2];
    let c3 = &grid.children[3];

    // Width = (420 - 20) / 2 = 200px
    assert!((c0.rect.width - 200.0).abs() < 1.0);
    assert!((c1.rect.width - 200.0).abs() < 1.0);
    assert_eq!(c0.rect.y, c1.rect.y);
    assert_eq!(c2.rect.y, c3.rect.y);
    assert!(c2.rect.y > c0.rect.y);
}

#[test]
fn test_table_formatting_context() {
    let html = r#"
        <table class="wikitable">
            <thead>
                <tr><th>Header 1</th><th>Header 2</th></tr>
            </thead>
            <tbody>
                <tr><td>Data 1</td><td>Data 2</td></tr>
                <tr><td colspan="2">Spanned across 2 columns</td></tr>
            </tbody>
        </table>
    "#;
    let css = r#"
        .wikitable { width: 500px; }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let mut layout = create_layout_tree(&dom, &rules, 800).unwrap();
    perform_layout(&mut layout, 800.0);

    let table = find_by_class(&layout, "wikitable").unwrap();
    assert_eq!(table.rect.display, DisplayType::Table);
    assert_eq!(table.children.len(), 2); // thead, tbody

    let thead = &table.children[0];
    let th1 = &thead.children[0].children[0];
    let th2 = &thead.children[0].children[1];

    assert!((th1.rect.width - 250.0).abs() < 1.0);
    assert!((th2.rect.width - 250.0).abs() < 1.0);
    assert!(th2.rect.x > th1.rect.x);

    let tbody = &table.children[1];
    let spanned_cell = &tbody.children[1].children[0];
    assert!(
        (spanned_cell.rect.width - 500.0).abs() < 1.0,
        "colspan=2 must occupy full table width"
    );
}

#[test]
fn test_stacking_contexts_and_z_index_order() {
    let html = r#"
        <div class="pos-top">Positive Z-Index</div>
        <div class="pos-bottom">Negative Z-Index</div>
        <div class="in-flow">In-flow Block</div>
    "#;
    let css = r#"
        .pos-top { position: relative; z-index: 100; background-color: rgb(255, 0, 0); height: 40px; }
        .pos-bottom { position: relative; z-index: -10; background-color: rgb(0, 0, 255); height: 40px; }
        .in-flow { background-color: rgb(0, 255, 0); height: 40px; }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let layout = create_layout_tree(&dom, &rules, 800).unwrap();
    let display_list = build_display_list(&layout);

    let mut neg_z_idx = None;
    let mut in_flow_idx = None;
    let mut pos_z_idx = None;

    for (i, item) in display_list.items.iter().enumerate() {
        if let DisplayItem::Rect { color, .. } = item {
            if color.b > 0.9 && color.r < 0.1 {
                neg_z_idx = Some(i);
            } else if color.g > 0.9 && color.r < 0.1 {
                in_flow_idx = Some(i);
            } else if color.r > 0.9 && color.g < 0.1 {
                pos_z_idx = Some(i);
            }
        }
    }

    assert!(neg_z_idx.is_some() && in_flow_idx.is_some() && pos_z_idx.is_some());
    assert!(
        neg_z_idx.unwrap() < in_flow_idx.unwrap(),
        "Negative z-index must paint before in-flow blocks"
    );
    assert!(
        in_flow_idx.unwrap() < pos_z_idx.unwrap(),
        "In-flow blocks must paint before positive z-index"
    );
}

#[test]
fn test_visual_effects_and_decorations() {
    let html = r#"
        <p class="capitalized">hello world</p>
        <p class="shadow-box">Box with shadow</p>
    "#;
    let css = r#"
        .capitalized { text-transform: capitalize; }
        .shadow-box { box-shadow: 2px 2px 4px rgba(0,0,0,0.5); }
    "#;
    let dom = parse_html(html);
    let rules = parse_css(css);
    let layout = create_layout_tree(&dom, &rules, 800).unwrap();
    let display_list = build_display_list(&layout);

    let capitalized_run = display_list
        .items
        .iter()
        .find(|i| matches!(i, DisplayItem::TextRun { content, .. } if content == "Hello World"));
    assert!(
        capitalized_run.is_some(),
        "text-transform: capitalize must transform 'hello world' into 'Hello World'"
    );

    let metrics = calculate_visible_metrics(&display_list, 800.0, 600.0);
    assert!(metrics.visible_text_characters > 10);
    assert!(metrics.meaningful_item_count >= 2);
    assert!(!metrics.has_major_blank_region);
    assert!(metrics.completeness_score > 0.1);
}

#[test]
fn test_wikipedia_sample_layout_and_paint() {
    let html = include_str!("fixtures/layout/wikipedia_layout_sample.html");
    let dom = parse_html(html);

    let mut rules = Vec::new();
    for style_elem in dom.find_all_tags("style") {
        let css_text = style_elem.text.trim();
        if !css_text.is_empty() {
            rules.append(&mut parse_css(css_text));
        }
    }

    let mut layout = create_layout_tree(&dom, &rules, 1024).expect("layout tree created");
    perform_layout(&mut layout, 1024.0);

    // Verify header layout
    let header = find_by_class(&layout, "header").expect("header element");
    assert_eq!(header.rect.display, DisplayType::Flex);
    assert_eq!(header.rect.y, 0.0);

    // Verify sidebar & content layout
    let main_container = find_by_class(&layout, "main-container").expect("main container");
    assert_eq!(main_container.rect.display, DisplayType::Grid);

    let sidebar = find_by_class(&layout, "sidebar").expect("sidebar element");
    let content = find_by_class(&layout, "content").expect("content element");
    assert!(
        content.rect.x > sidebar.rect.x,
        "Content must sit to the right of sidebar without overlap"
    );

    // Verify infobox float
    let infobox = find_by_class(&layout, "infobox").expect("infobox element");
    assert_eq!(infobox.computed_style.float.as_deref(), Some("right"));
    assert!(infobox.rect.x > content.rect.x);

    // Verify table layout
    let table = find_by_class(&layout, "wikitable").expect("wikitable element");
    assert_eq!(table.rect.display, DisplayType::Table);

    // Verify footer clearance
    let footer = find_by_class(&layout, "footer").expect("footer element");
    assert!(
        footer.rect.y >= main_container.rect.y + main_container.rect.height,
        "Footer must sit below main container"
    );

    // Build display list and verify visible metrics
    let display_list = build_display_list(&layout);
    assert!(!display_list.is_empty());
    assert!(
        display_list.links.len() >= 5,
        "Language links must have registered clickable regions"
    );

    let metrics = calculate_visible_metrics(&display_list, 1024.0, 800.0);
    assert!(metrics.visible_text_characters > 100);
    assert!(!metrics.has_major_blank_region);
    assert!(metrics.completeness_score > 0.5);
}
