/// Returns path: {data_dir}/opc-desktop/company.md
pub fn company_context_path() -> std::path::PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    base.join("opc-desktop").join("company.md")
}

/// Read the file; returns None if missing or empty.
pub fn read_company_context() -> Option<String> {
    let path = company_context_path();
    let text = std::fs::read_to_string(&path).ok()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Write (create dir if needed).
pub fn write_company_context(text: &str) -> Result<(), String> {
    let path = company_context_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())
}
