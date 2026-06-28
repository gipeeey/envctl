use indexmap::IndexMap;

pub fn parse(content: &str) -> IndexMap<String, String> {
    let mut map = IndexMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim().to_string();
            let v = parse_value(v.trim());
            map.insert(k, v);
        }
    }
    map
}

fn parse_value(v: &str) -> String {
    if let Some(inner) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        inner.replace("\\\"", "\"").replace("\\\\", "\\")
    } else if let Some(inner) = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        inner.to_string()
    } else {
        v.to_string()
    }
}

pub fn serialize(vars: &IndexMap<String, String>) -> String {
    let mut out = String::new();
    for (k, v) in vars {
        let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!("{k}=\"{escaped}\"\n"));
    }
    out
}
