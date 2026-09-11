use super::parser::Element;

pub fn render_to_string(element: &Element) -> String {
    const MAX_RENDER_DEPTH: usize = 512;
    const MAX_RENDER_BYTES: usize = 2 * 1024 * 1024;
    let mut output = String::new();
    // Iterative pre-order walk with explicit open/close tasks so deep DOMs
    // cannot overflow the stack. Children are pushed in reverse to preserve
    // document order; close tags are emitted after their children.
    enum Task<'a> {
        Open(&'a Element, usize, usize),
        Close(&'a str),
    }
    let mut stack: Vec<Task<'_>> = vec![Task::Open(element, 0, 0)];
    let mut truncated = false;
    while let Some(task) = stack.pop() {
        if output.len() >= MAX_RENDER_BYTES {
            truncated = true;
            break;
        }
        match task {
            Task::Close(tag) => {
                output.push_str(&format!("</{}>", tag));
            }
            Task::Open(el, indent, depth) => {
                if depth > MAX_RENDER_DEPTH {
                    continue;
                }
                let space = "  ".repeat(indent);
                match el.tag.as_str() {
                    "img" => {
                        if let Some(src) = el.get_attr("src") {
                            output.push_str(&format!("{}📷 [Image: {}]", space, src));
                        } else {
                            output.push_str(&format!("{}📷 [Image]", space));
                        }
                    }
                    "title" => {
                        output.push_str(&format!("{}📌 [Title: {}]", space, el.text));
                    }
                    "a" => {
                        if let Some(href) = el.get_attr("href") {
                            if !el.text.is_empty() {
                                output.push_str(&format!("{}🔗 [{}]({})", space, el.text, href));
                            } else {
                                output.push_str(&format!("{}🔗 [{}]", space, href));
                            }
                        } else {
                            output.push_str(&format!("{}🔗 [link]", space));
                        }
                        // Mixed content (links inside the anchor) must not be dropped.
                        for child in el.children.iter().rev() {
                            stack.push(Task::Open(child, indent + 1, depth + 1));
                        }
                    }
                    "p" => {
                        output.push_str(&format!("{}✎ {}", space, el.text));
                        for child in el.children.iter().rev() {
                            stack.push(Task::Open(child, indent + 1, depth + 1));
                        }
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        let level = el.tag.trim_start_matches('h').parse::<u32>().unwrap_or(1);
                        let marker = "#".repeat((level as usize).min(6));
                        output.push_str(&format!("{}{} {}", space, marker, el.text));
                        for child in el.children.iter().rev() {
                            stack.push(Task::Open(child, indent + 1, depth + 1));
                        }
                    }
                    _ => {
                        if !el.children.is_empty() {
                            output.push_str(&format!("{}<{}>", space, el.tag));
                            stack.push(Task::Close(el.tag.as_str()));
                            for child in el.children.iter().rev() {
                                stack.push(Task::Open(child, indent + 1, depth + 1));
                            }
                        } else if !el.text.is_empty() {
                            output.push_str(&format!("{}<{}>: {}", space, el.tag, el.text));
                        }
                    }
                }
            }
        }
    }
    if truncated {
        output.push_str("…[truncated: output cap reached]");
    }
    output
}

#[allow(dead_code)]
fn render_element(element: &Element, indent: usize) -> String {
    render_to_string_bounded(element, indent)
}

/// Depth-limited recursive helper kept for the unit below; production
/// `render_to_string` uses the iterative walk above.
fn render_to_string_bounded(element: &Element, indent: usize) -> String {
    render_element_capped(element, indent, 0)
}

fn render_element_capped(element: &Element, indent: usize, depth: usize) -> String {
    const MAX_RENDER_DEPTH: usize = 512;
    if depth > MAX_RENDER_DEPTH {
        return String::new();
    }
    let mut output = String::new();
    let space = "  ".repeat(indent);

    match element.tag.as_str() {
        "img" => {
            if let Some(src) = element.get_attr("src") {
                output.push_str(&format!("{}📷 [Image: {}]", space, src));
            } else {
                output.push_str(&format!("{}📷 [Image]", space));
            }
        }
        "title" => {
            output.push_str(&format!("{}📌 [Title: {}]", space, element.text));
        }
        "a" => {
            if let Some(href) = element.get_attr("href") {
                if !element.text.is_empty() {
                    output.push_str(&format!("{}🔗 [{}]({})", space, element.text, href));
                } else {
                    output.push_str(&format!("{}🔗 [{}]", space, href));
                }
            } else {
                output.push_str(&format!("{}🔗 [link]", space));
            }
            // Mixed content (links inside the anchor) must not be dropped.
            for child in &element.children {
                output.push_str(&render_element_capped(child, indent + 1, depth + 1));
            }
        }
        "p" => {
            output.push_str(&format!("{}✎ {}", space, element.text));
            render_children_into_capped(&mut output, element, indent, depth);
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = element
                .tag
                .trim_start_matches('h')
                .parse::<u32>()
                .unwrap_or(1);
            let marker = "#".repeat((level as usize).min(6));
            output.push_str(&format!("{}{} {}", space, marker, element.text));
            render_children_into_capped(&mut output, element, indent, depth);
        }
        _ => {
            if !element.children.is_empty() {
                output.push_str(&format!("{}<{}>", space, element.tag));

                for child in &element.children {
                    output.push_str(&render_element_capped(child, indent + 1, depth + 1));
                }

                output.push_str(&format!("</{}>", element.tag));
            } else if !element.text.is_empty() {
                output.push_str(&format!("{}<{}>: {}", space, element.tag, element.text));
            }
        }
    }

    output
}

fn render_children_into_capped(
    output: &mut String,
    element: &Element,
    indent: usize,
    depth: usize,
) {
    for child in &element.children {
        output.push_str(&render_element_capped(child, indent + 1, depth + 1));
    }
}

#[allow(dead_code)]
fn render_children_into(output: &mut String, element: &Element, indent: usize) {
    render_children_into_capped(output, element, indent, 0);
}

pub fn count_images(element: &Element) -> usize {
    // Iterative stack walk with a depth limit so adversarial DOMs cannot
    // overflow the stack; output is a bounded count.
    const MAX_COUNT_DEPTH: usize = 4096;
    let mut count = 0usize;
    let mut stack: Vec<(&Element, usize)> = vec![(element, 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > MAX_COUNT_DEPTH {
            continue;
        }
        if node.tag == "img" {
            count = count.saturating_add(1);
            if count >= 1_000_000 {
                break;
            }
        }
        for child in &node.children {
            stack.push((child, depth + 1));
        }
    }
    count
}
