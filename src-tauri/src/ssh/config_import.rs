use crate::{
    error::{AppError, AppResult},
    models::Connection,
};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct Imported {
    pub connections: Vec<Connection>,
    pub warnings: Vec<String>,
}
#[derive(Default)]
struct Block {
    patterns: Vec<String>,
    fields: HashMap<String, String>,
}
pub fn parse(text: &str, home: &str, default_user: &str) -> AppResult<Imported> {
    let mut blocks = vec![Block {
        patterns: vec!["*".into()],
        ..Default::default()
    }];
    let mut warnings = Vec::new();
    let mut skip_match = false;
    for (n, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
        let tokens = tokenize(line)
            .map_err(|m| AppError::new("E_SSH_CONFIG", "import", format!("Line {}: {m}", n + 1)))?;
        if tokens.is_empty() {
            continue;
        }
        let field = tokens[0].to_ascii_lowercase();
        let values = &tokens[1..];
        if field == "host" {
            blocks.push(Block {
                patterns: values.to_vec(),
                ..Default::default()
            });
            skip_match = false;
            continue;
        }
        if field == "match" {
            warnings.push(format!("Line {}: Match blocks are not imported", n + 1));
            skip_match = true;
            continue;
        }
        if skip_match {
            continue;
        }
        if field == "include" {
            warnings.push(format!(
                "Line {}: Include files must be imported separately",
                n + 1
            ));
            continue;
        }
        if ["hostname", "port", "user", "identityfile", "proxyjump"].contains(&field.as_str()) {
            if values.len() != 1 {
                return Err(AppError::new(
                    "E_SSH_CONFIG",
                    "import",
                    format!("Line {}: expected one value", n + 1),
                ));
            }
            blocks
                .last_mut()
                .unwrap()
                .fields
                .entry(field)
                .or_insert_with(|| values[0].clone());
        }
    }
    let mut names = Vec::new();
    for b in &blocks {
        for p in &b.patterns {
            if !p.contains(['*', '?', '!']) && !names.contains(p) {
                names.push(p.clone());
            }
        }
    }
    let mut connections = Vec::new();
    for name in names {
        let mut fields = HashMap::new();
        for b in &blocks {
            if block_matches(&b.patterns, &name) {
                for (k, v) in &b.fields {
                    fields.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
        let user = fields
            .get("user")
            .cloned()
            .unwrap_or_else(|| default_user.into());
        let host = fields
            .get("hostname")
            .cloned()
            .unwrap_or_else(|| name.clone())
            .replace("%h", &name);
        let port = fields
            .get("port")
            .map(|p| p.parse::<u16>())
            .transpose()
            .map_err(|_| {
                AppError::new(
                    "E_SSH_CONFIG",
                    "import",
                    format!("Host {name}: invalid port"),
                )
            })?
            .unwrap_or(22);
        let key_path = fields
            .get("identityfile")
            .map(|p| {
                p.replace("%d", home)
                    .replace("%h", &host)
                    .replace("%n", &name)
                    .replace("%r", &user)
                    .replace("%p", &port.to_string())
            })
            .map(|p| {
                if let Some(relative) = p.strip_prefix("~/") {
                    format!("{home}/{relative}")
                } else {
                    p
                }
            });
        connections.push(Connection {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            host,
            port,
            user,
            auth_method: if key_path.is_some() { "key" } else { "agent" }.into(),
            key_path,
            agent_kind: Some("openssh".into()),
            proxy_jump: fields
                .get("proxyjump")
                .filter(|p| p.as_str() != "none")
                .cloned(),
            source: Some("~/.ssh/config".into()),
            last_used_at: None,
        });
    }
    Ok(Imported {
        connections,
        warnings,
    })
}
fn tokenize(line: &str) -> Result<Vec<String>, &'static str> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if escaped {
            word.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' {
            if chars
                .peek()
                .is_some_and(|next| next.is_whitespace() || ['\\', '\'', '"', '#'].contains(next))
            {
                escaped = true;
            } else {
                word.push(c);
            }
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            continue;
        }
        if c == '#' {
            break;
        }
        if c.is_whitespace() || (c == '=' && words.len() < 2) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if quote.is_some() {
        return Err("Unterminated quoted value");
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}
fn block_matches(patterns: &[String], name: &str) -> bool {
    !patterns
        .iter()
        .filter_map(|p| p.strip_prefix('!'))
        .any(|p| wildcard(p, name))
        && patterns
            .iter()
            .filter(|p| !p.starts_with('!'))
            .any(|p| wildcard(p, name))
}
fn wildcard(pattern: &str, value: &str) -> bool {
    let p = pattern.to_ascii_lowercase().into_bytes();
    let v = value.to_ascii_lowercase().into_bytes();
    let mut previous = vec![false; v.len() + 1];
    previous[0] = true;
    for c in p {
        let mut next = vec![false; v.len() + 1];
        if c == b'*' {
            next[0] = previous[0];
        }
        for i in 1..=v.len() {
            next[i] = if c == b'*' {
                previous[i] || next[i - 1]
            } else {
                previous[i - 1] && (c == b'?' || c == v[i - 1])
            };
        }
        previous = next;
    }
    previous[v.len()]
}
pub fn normalize_endpoint(
    value: &str,
    default_user: &str,
    default_port: u16,
) -> AppResult<(String, String, u16)> {
    let (user, endpoint) = value
        .rsplit_once('@')
        .map(|(u, h)| (u.to_string(), h))
        .unwrap_or_else(|| (default_user.into(), value));
    let (host, port) = if let Some(bracket) = endpoint.strip_prefix('[') {
        let (host, suffix) = bracket
            .split_once(']')
            .ok_or_else(|| AppError::new("E_INPUT", "connection", "Invalid IPv6 address"))?;
        let port = if suffix.is_empty() {
            default_port
        } else {
            suffix
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok())
                .ok_or_else(|| AppError::new("E_INPUT", "connection", "Invalid port"))?
        };
        (host.to_owned(), port)
    } else if endpoint.matches(':').count() == 1 {
        let (h, p) = endpoint.rsplit_once(':').unwrap();
        (
            h.into(),
            p.parse()
                .map_err(|_| AppError::new("E_INPUT", "connection", "Invalid port"))?,
        )
    } else {
        (endpoint.to_owned(), default_port)
    };
    if host.is_empty() || user.is_empty() || port == 0 {
        return Err(AppError::new(
            "E_INPUT",
            "connection",
            "Host, user and port are required",
        ));
    }
    Ok((host, user, port))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_key_paths_and_utf8_bom_are_preserved() {
        let imported = parse(
            "\u{feff}Host windows-key\n IdentityFile \"C:\\keys\\id_ed25519\"\n",
            "/home/test",
            "fixture",
        )
        .unwrap();
        assert_eq!(
            imported.connections[0].key_path.as_deref(),
            Some(r"C:\keys\id_ed25519")
        );
    }
    #[test]
    fn imports_quotes_defaults_first_value_and_proxy() {
        let data=parse("Host prod alias\n HostName example.org\n User dev\n Port=2222\n IdentityFile \"~/.ssh/a key\"\n ProxyJump gateway\nHost *\n User ignored\n", "/home/test","fallback").unwrap();
        assert_eq!(data.connections.len(), 2);
        let c = &data.connections[0];
        assert_eq!(c.host, "example.org");
        assert_eq!(c.port, 2222);
        assert_eq!(c.user, "dev");
        assert_eq!(c.key_path.as_deref(), Some("/home/test/.ssh/a key"));
        assert_eq!(c.proxy_jump.as_deref(), Some("gateway"));
    }
    #[test]
    fn wildcard_negation_and_include_are_explicit() {
        let result=parse("Include extra\nHost * !private\n User common\nHost public private\n HostName example\n", "/home/test","fallback").unwrap();
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.connections[0].user, "common");
        assert_eq!(result.connections[1].user, "fallback");
    }
    #[test]
    fn endpoint_supports_ipv6_and_inline_user() {
        assert_eq!(
            normalize_endpoint("dev@[2001:db8::1]:2222", "u", 22).unwrap(),
            ("2001:db8::1".into(), "dev".into(), 2222)
        );
        assert_eq!(
            normalize_endpoint("2001:db8::1", "u", 22).unwrap().0,
            "2001:db8::1"
        );
        assert!(normalize_endpoint("host:0", "u", 22).is_err());
    }
}
