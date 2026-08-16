//! Phase 21 acceptance gate: bounded Canvas 2D and SVG vector-shape host
//! capabilities. Canvas shapes drawn through the 2D context and SVG shape
//! elements must appear in the rendered display list, bounded and fail-closed.

use ghitabrowser::paint::{DisplayItem, VectorShapeKind};
use ghitabrowser::web_runtime::PageRuntime;

fn page(html: &str) -> PageRuntime {
    let mut page = PageRuntime::from_html(html, Vec::new(), 800, "https://app.test/")
        .expect("page runtime construction must succeed");
    page.run_document().expect("inline scripts must run");
    page
}

#[test]
fn canvas_2d_fill_rect_appears_in_the_display_list() {
    let mut page = page(
        "<main><canvas id='c' width='200' height='100'></canvas>\
         <script>let c=document.getElementById('c');let ctx=c.getContext('2d');\
         ctx.fillStyle='#ff0000';ctx.fillRect(10,10,50,30);\
         </script></main>",
    );
    let render = page.refresh_render();
    let shapes: Vec<&DisplayItem> = render
        .display_list
        .items
        .iter()
        .filter(|item| matches!(item, DisplayItem::VectorShape(_)))
        .collect();
    assert_eq!(shapes.len(), 1, "one canvas rect expected: {:?}", shapes);
    let DisplayItem::VectorShape(shape) = shapes[0] else {
        unreachable!()
    };
    assert_eq!(shape.kind, VectorShapeKind::Rect);
    assert_eq!(shape.w, 50.0);
    assert_eq!(shape.h, 30.0);
    assert_eq!(shape.fill.unwrap().r, 1.0, "fillStyle must apply");
    assert!(
        page.report().errors.is_empty(),
        "{:?}",
        page.report().errors
    );
}

#[test]
fn canvas_style_and_clear_round_trip_through_the_context() {
    let mut page = page(
        "<main><canvas id='c'></canvas>\
         <script>let ctx=document.getElementById('c').getContext('2d');\
         ctx.fillStyle='#00ff00';let style=ctx.fillStyle;\
         ctx.fillRect(0,0,10,10);\
         ctx.clearRect(0,0,100,100);\
         </script></main>",
    );
    assert_eq!(
        page.evaluate("style"),
        Ok(ghitabrowser::javascript::JsvValue::String(
            "#00ff00".to_string()
        ))
    );
    let render = page.refresh_render();
    assert!(
        render
            .display_list
            .items
            .iter()
            .all(|item| !matches!(item, DisplayItem::VectorShape(_))),
        "clearRect must remove canvas shapes"
    );
}

#[test]
fn svg_shapes_render_with_attributes() {
    let page = page(
        "<main><svg width='200' height='100'>\
         <rect x='5' y='10' width='40' height='20' fill='blue'></rect>\
         <circle cx='100' cy='50' r='25' fill='red'></circle>\
         <line x1='0' y1='0' x2='30' y2='40' stroke='black'></line>\
         </svg></main>",
    );
    let render = page.live_document().render_state();
    let shapes: Vec<&DisplayItem> = render
        .display_list
        .items
        .iter()
        .filter(|item| matches!(item, DisplayItem::VectorShape(_)))
        .collect();
    assert_eq!(shapes.len(), 3, "rect + circle + line: {:?}", shapes);
    let DisplayItem::VectorShape(rect) = shapes[0] else {
        unreachable!()
    };
    assert_eq!(rect.kind, VectorShapeKind::Rect);
    assert_eq!(rect.w, 40.0);
    assert_eq!(rect.h, 20.0);
    assert_eq!(rect.fill.unwrap().b, 1.0);
    let DisplayItem::VectorShape(circle) = shapes[1] else {
        unreachable!()
    };
    assert_eq!(circle.kind, VectorShapeKind::Ellipse);
    assert_eq!(circle.w, 50.0, "circle diameter from r=25");
}

#[test]
fn unsupported_canvas_contexts_and_capabilities_fail_closed() {
    // "3d" contexts are not implemented: getContext returns null.
    let mut page_a = page(
        "<main><canvas id='c'></canvas>\
         <script>let ctx=document.getElementById('c').getContext('webgl');let isNull=ctx===null;\
         </script></main>",
    );
    assert_eq!(
        page_a.evaluate("isNull"),
        Ok(ghitabrowser::javascript::JsvValue::Boolean(true))
    );
    // WebAssembly is not an implemented capability: the global is absent.
    let mut page2 = page(
        "<main><script>let present=typeof WebAssembly;\
         </script></main>",
    );
    assert_eq!(
        page2.evaluate("present"),
        Ok(ghitabrowser::javascript::JsvValue::String(
            "undefined".to_string()
        ))
    );
}

#[test]
fn canvas_shape_budgets_are_bounded() {
    let script = "let ctx=document.getElementById('c').getContext('2d');\
                  let i=0;while(i<1000){ctx.fillRect(0,0,1,1);i=i+1}";
    let mut page = page(&format!(
        "<main><canvas id='c'></canvas><script>{script}</script></main>"
    ));
    let render = page.refresh_render();
    let count = render
        .display_list
        .items
        .iter()
        .filter(|item| matches!(item, DisplayItem::VectorShape(_)))
        .count();
    assert!(
        count <= 512,
        "shape budget must bound the display list: {count}"
    );
}
