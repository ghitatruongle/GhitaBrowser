

use super::parser::Element;

pub fn render_to_string(element: &Element) -> String {
    render_element(element, 0)
}

fn render_element(element: &Element, indent: usize) -> String {
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
        }
        "p" => {
            output.push_str(&format!("{}✎ {}", space, element.text));
        }
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = element
                .tag
                .trim_start_matches('h')
                .parse::<u32>()
                .unwrap_or(1);
            let marker = "#".repeat((level as usize).min(6));
            output.push_str(&format!("{}{} {}", space, marker, element.text));
        }
        _ => {
            if !element.children.is_empty() {
                output.push_str(&format!("{}<{}>", space, element.tag));

                for child in &element.children {
                    output.push_str(&render_element(child, indent + 1));
                }

                output.push_str(&format!("</{}>", element.tag));
            } else if !element.text.is_empty() {
                output.push_str(&format!("{}<{}>: {}", space, element.tag, element.text));
            }
        }
    }

    output
}

pub fn count_images(element: &Element) -> usize {
    let mut count = 0;
    if element.tag == "img" {
        count = 1;
    }
    for child in &element.children {
        count += count_images(child);
    }
    count
}
