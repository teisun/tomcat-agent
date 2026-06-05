use super::super::markdownify::render_textual_body;
use super::super::types::WebFetchFormat;

#[test]
fn html_with_script_style_and_nav_stripped() {
    let html = r#"
        <html>
          <head>
            <style>.danger { color: red; }</style>
            <script>console.log("secret")</script>
          </head>
          <body>
            <nav>top nav</nav>
            <article><h1>Hello</h1><p>World</p></article>
          </body>
        </html>
    "#;
    let out = render_textual_body(
        html.as_bytes(),
        "text/html; charset=utf-8",
        WebFetchFormat::Markdown,
    );
    assert!(out.contains("Hello"));
    assert!(out.contains("World"));
    assert!(!out.contains("console.log"));
    assert!(!out.contains("top nav"));
    assert!(!out.contains("color: red"));
}

#[test]
fn html_with_table_kept() {
    let html = r#"
        <html>
          <body>
            <table>
              <tr><th>Name</th><th>Value</th></tr>
              <tr><td>alpha</td><td>1</td></tr>
            </table>
          </body>
        </html>
    "#;
    let out = render_textual_body(
        html.as_bytes(),
        "text/html; charset=utf-8",
        WebFetchFormat::Markdown,
    );
    assert!(out.contains("alpha"));
    assert!(out.contains("Value"));
}

#[test]
fn markdown_format_text_mode_strips_basic_markup() {
    let markdown = "# Title\n\n- item\n\n[link](https://example.com)";
    let out = render_textual_body(markdown.as_bytes(), "text/markdown", WebFetchFormat::Text);
    assert!(out.contains("Title"));
    assert!(out.contains("item"));
    assert!(out.contains("link"));
    assert!(!out.contains("[link]("));
}

#[test]
fn json_content_is_returned_verbatim() {
    let json = "{\r\n  \"title\": \"demo\"\r\n\r\n}\r\n";
    let out = render_textual_body(
        json.as_bytes(),
        "application/json",
        WebFetchFormat::Markdown,
    );
    assert_eq!(out, json);
}

#[test]
fn xml_content_is_returned_verbatim_for_plus_xml_types() {
    let xml = "<feed>\r\n\r\n  <title>demo</title>\r\n</feed>\r\n";
    let out = render_textual_body(xml.as_bytes(), "application/atom+xml", WebFetchFormat::Text);
    assert_eq!(out, xml);
}
