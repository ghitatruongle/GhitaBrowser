use ghitabrowser::css_parser::parse_css;
use ghitabrowser::layout::{create_layout_tree, LayoutNode};
use ghitabrowser::paint::{DisplayItem, Rgba};
use ghitabrowser::{DynamicFrameBudget, LiveDocument};

fn find_by_id<'a>(node: &'a LayoutNode, id: &str) -> Option<&'a LayoutNode> {
    if node.element.get_attr("id").map(String::as_str) == Some(id) {
        return Some(node);
    }
    node.children.iter().find_map(|child| find_by_id(child, id))
}

fn find_by_display<'a>(node: &'a LayoutNode, display: &str) -> Option<&'a LayoutNode> {
    if node.computed_style.display.as_deref() == Some(display) {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_by_display(child, display))
}

#[test]
fn phase14_paint_mutation_reuses_layout_and_retains_clean_display_list() {
    let rules = parse_css(
        "#panel { width: 320px; background-color: #102030; color: white; }\
         #panel > p { color: white; }",
    );
    let mut document = LiveDocument::parse(
        "<section id='panel' style='opacity:1;transition: opacity 100ms'><p id='message'>before</p></section>",
        rules,
        800,
    );
    let panel = document.get_element_by_id("panel").unwrap();
    let layout_before = document.render_state().layout.clone().unwrap();
    let rect_before = find_by_id(&layout_before, "panel").unwrap().rect;

    document
        .set_attribute(
            panel,
            "style",
            "opacity:0.2;transition: opacity 100ms;background-color:#ff0000",
        )
        .unwrap();
    let changed = document.refresh().clone();
    assert!(changed.dynamic.layout_reused);
    assert!(changed.dynamic.cascade_cache_hits > 0);
    let rect_after = find_by_id(changed.layout.as_ref().unwrap(), "panel")
        .unwrap()
        .rect;
    assert_eq!(rect_before.width, rect_after.width);
    assert_eq!(rect_before.height, rect_after.height);

    document.advance_animations(50);
    let animated = document.render_state();
    assert!(animated.dynamic.active_animations > 0);
    assert!(animated.display_list.items.iter().any(|item| {
        matches!(item, DisplayItem::Rect { color, .. } if color.r > 0.9 && color.a < 1.0)
    }));

    document.advance_animations(100);
    let settled = document.refresh();
    assert!(settled.dynamic.display_list_reused);
    assert!(settled.display_list.items.iter().any(|item| {
        matches!(item, DisplayItem::Rect { color, .. } if *color == Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 0.2
        })
    }));
}

#[test]
fn phase14_positioned_transform_flex_and_grid_layout_are_bounded() {
    let html = "
        <main class='relative'>
          <div class='flex'>
            <p id='small' class='one'>one</p><p id='large' class='two'>two</p>
          </div>
          <div class='grid'><p>one</p><p>two</p><p>three</p></div>
          <p id='badge' class='absolute'>badge</p>
        </main>";
    let rules = parse_css(
        ".relative{position:relative;width:360px;height:220px;transform:translateX(20px)}\
         .absolute{position:absolute;left:40px;top:150px;background-color:#ff0000}\
         .flex{display:flex;width:300px;gap:10px;justify-content:center;align-items:center}\
         .one{flex:1 1 0}.two{flex:2 1 0}\
         .grid{display:grid;width:300px;grid-template-columns:repeat(2, 1fr);grid-template-rows:50px 50px;gap:8px}",
    );
    let layout = create_layout_tree(&ghitabrowser::parse_html(html), &rules, 800).unwrap();
    let small = find_by_id(&layout, "small").unwrap();
    let large = find_by_id(&layout, "large").unwrap();
    let badge = find_by_id(&layout, "badge").unwrap();
    assert!(large.rect.width > small.rect.width * 1.8);
    assert!(badge.rect.x >= 60.0, "parent transform + left offset");
    assert!(badge.rect.y >= 150.0);

    let grid = find_by_display(&layout, "grid").unwrap();
    assert!(grid.children[1].rect.x > grid.children[0].rect.x);
    assert!(grid.children[2].rect.y > grid.children[0].rect.y);
}

#[test]
fn phase14_mutation_frames_stay_inside_retained_memory_and_time_budgets() {
    let cards = (0..240)
        .map(|index| format!("<p class='card' id='card-{index}'>card {index}</p>"))
        .collect::<String>();
    let mut document = LiveDocument::parse(
        &format!("<main>{cards}</main>"),
        parse_css(".card{padding:2px;background-color:#202020;color:#ffffff}"),
        900,
    );
    let budget = DynamicFrameBudget::default();
    for index in 0..120 {
        let node = document
            .get_element_by_id(&format!("card-{}", index % 240))
            .unwrap();
        let color = if index % 2 == 0 { "#303030" } else { "#404040" };
        document
            .set_attribute(
                node,
                "style",
                &format!("background-color:{color};opacity:0.9"),
            )
            .unwrap();
        let frame = document.refresh().clone();
        let evaluation = budget.evaluate(&frame.dynamic);
        assert!(evaluation.passed(), "{:?}", evaluation.violations);
        assert!(frame.dynamic.layout_reused);
    }
}
