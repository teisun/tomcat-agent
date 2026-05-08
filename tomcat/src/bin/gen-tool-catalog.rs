use tomcat::core::tools::contract::catalog::render_tool_catalog_markdown;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rendered = render_tool_catalog_markdown();
    if std::env::var_os("UPDATE_TOOL_CATALOG").is_some() {
        std::fs::write("docs/tool-catalog.md", rendered)?;
    } else {
        print!("{}", rendered);
    }
    Ok(())
}
