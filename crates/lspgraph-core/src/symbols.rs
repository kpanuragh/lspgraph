//! Enumerate named callable declarations.
//!
//! `documentSymbol` is not a list of callables. tsserver synthesizes entries
//! for anonymous callbacks (`expect() callback`, `test("x") callback`), which
//! `prepareCallHierarchy` correctly refuses. Crawling them is pure waste.

use lsp_types::{DocumentSymbol, Position, SymbolKind, Url};

#[derive(Debug, Clone, PartialEq)]
pub struct NamedCallable {
    pub name: String,
    pub uri: Url,
    pub position: Position,
}

/// Spec 5.3: kind is Function or Method, and the name is a valid identifier.
/// First character must be alphabetic (Unicode), underscore, or dollar sign.
/// Remaining characters must be alphanumeric (Unicode), underscore, or dollar sign.
pub fn is_named_callable(name: &str, kind: SymbolKind) -> bool {
    if kind != SymbolKind::FUNCTION && kind != SymbolKind::METHOD {
        return false;
    }

    // Servers qualify method names in their own ways, and all of them are real,
    // findable methods: gopls returns `counter.bump` from `workspace/symbol`
    // and `(*counter).bump` from `documentSymbol`. Strip a leading
    // parenthesised receiver, then require every dot-separated segment to be a
    // plain identifier.
    //
    // The segment rule is what keeps tsserver's synthesized callback names out:
    // `_def.checks.find() callback` is dotted too, but its last segment carries
    // parentheses and a space, so it still fails.
    let rest = match name.strip_prefix('(') {
        Some(after) => {
            let Some(close) = after.find(')') else {
                return false;
            };
            let receiver = after[..close].strip_prefix('*').unwrap_or(&after[..close]);
            if !is_plain_identifier(receiver) {
                return false;
            }
            after[close + 1..].strip_prefix('.').unwrap_or("")
        }
        None => name,
    };

    !rest.is_empty() && rest.split('.').all(is_plain_identifier)
}

/// A single unqualified identifier segment.
fn is_plain_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '_' || first == '$') {
        return false;
    }
    chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

pub fn collect_named_callables(
    symbols: &[DocumentSymbol],
    uri: &Url,
    out: &mut Vec<NamedCallable>,
) {
    for s in symbols {
        if is_named_callable(&s.name, s.kind) {
            out.push(NamedCallable {
                name: s.name.clone(),
                uri: uri.clone(),
                position: s.selection_range.start,
            });
        }
        if let Some(children) = &s.children {
            collect_named_callables(children, uri, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_identifiers() {
        for n in [
            "types",
            "handle_request",
            "_custom",
            "$ref",
            "catchall",
            "uuid",
        ] {
            assert!(
                is_named_callable(n, SymbolKind::FUNCTION),
                "should accept {n}"
            );
        }
    }

    #[test]
    fn rejects_synthesized_callback_names() {
        // Observed verbatim from tsserver during the feasibility probe.
        for n in [
            "expect() callback",
            "test(\"check any inference\") callback",
            "on(\"cycle\") callback",
            "_def.checks.find() callback",
            "patternKeys.map() callback",
            "z.custom() callback",
        ] {
            assert!(
                !is_named_callable(n, SymbolKind::FUNCTION),
                "should reject {n}"
            );
        }
    }

    #[test]
    fn accepts_qualified_method_names() {
        // gopls names a method `counter.bump` in workspace/symbol results and
        // `(*counter).bump` in documentSymbol. Both are real, findable methods.
        for n in [
            "counter.bump",
            "(*counter).bump",
            "(counter).bump",
            "pkg.Type.method",
        ] {
            assert!(
                is_named_callable(n, SymbolKind::METHOD),
                "should accept {n}"
            );
        }
    }

    #[test]
    fn a_qualified_name_still_rejects_callback_noise() {
        // The dotted-segment rule must not readmit tsserver's synthesized names,
        // which also contain dots.
        for n in [
            "_def.checks.find() callback",
            "patternKeys.map() callback",
            "z.custom() callback",
        ] {
            assert!(
                !is_named_callable(n, SymbolKind::FUNCTION),
                "should reject {n}"
            );
        }
    }

    #[test]
    fn rejects_empty_segments_and_stray_punctuation() {
        for n in [".bump", "counter.", "a..b", "(unclosed.bump", "()"] {
            assert!(
                !is_named_callable(n, SymbolKind::METHOD),
                "should reject {n}"
            );
        }
    }

    #[test]
    fn rejects_non_callable_kinds() {
        assert!(!is_named_callable("Config", SymbolKind::STRUCT));
        assert!(!is_named_callable("Color", SymbolKind::ENUM));
        assert!(!is_named_callable("MAX", SymbolKind::CONSTANT));
    }

    #[test]
    fn accepts_methods() {
        assert!(is_named_callable("pipe", SymbolKind::METHOD));
    }

    #[test]
    fn rejects_empty_and_leading_digit() {
        assert!(!is_named_callable("", SymbolKind::FUNCTION));
        assert!(!is_named_callable("2fast", SymbolKind::FUNCTION));
    }

    #[test]
    fn accepts_unicode_identifiers() {
        for n in [
            "validar_configuración",
            "Añadir",
            "日本語関数",
            "переменная",
        ] {
            assert!(
                is_named_callable(n, SymbolKind::FUNCTION),
                "should accept {n}"
            );
        }
    }

    #[allow(deprecated)]
    fn sym(
        name: &str,
        kind: SymbolKind,
        line: u32,
        children: Option<Vec<DocumentSymbol>>,
    ) -> DocumentSymbol {
        let r = lsp_types::Range {
            start: Position { line, character: 0 },
            end: Position {
                line,
                character: 10,
            },
        };
        DocumentSymbol {
            name: name.to_string(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range: r,
            selection_range: r,
            children,
        }
    }

    #[test]
    fn collects_recursively_and_filters() {
        let uri = Url::parse("file:///a.ts").unwrap();
        let tree = vec![sym(
            "Outer",
            SymbolKind::CLASS,
            1,
            Some(vec![
                sym("method", SymbolKind::METHOD, 2, None),
                sym("expect() callback", SymbolKind::FUNCTION, 3, None),
            ]),
        )];
        let mut out = Vec::new();
        collect_named_callables(&tree, &uri, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "method");
        assert_eq!(out[0].position.line, 2);
    }
}

/// The `SymbolKind` this crate treats as a plain function. Exposed so
/// downstream crates can build `SymbolMatch` values in tests without
/// depending on `lsp-types` directly.
pub fn function_kind() -> SymbolKind {
    SymbolKind::FUNCTION
}

/// A symbol found by a repository-wide `workspace/symbol` search.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolMatch {
    pub name: String,
    /// The enclosing class/module the server reported, if any.
    pub container: Option<String>,
    pub uri: Url,
    pub position: Position,
    pub kind: SymbolKind,
}

impl From<&SymbolMatch> for NamedCallable {
    fn from(m: &SymbolMatch) -> NamedCallable {
        NamedCallable {
            name: m.name.clone(),
            uri: m.uri.clone(),
            position: m.position,
        }
    }
}

/// Some servers decorate callable names in `workspace/symbol` results:
/// vtsls returns "middle()" where rust-analyzer returns "middle". Strip that
/// decoration so the stored name matches what call hierarchy reports.
///
/// Deliberately only a TRAILING "()": `documentSymbol`'s synthesized callback
/// names such as `expect() callback` do not end with it and must stay rejected.
fn undecorate(name: &str) -> &str {
    name.strip_suffix("()").unwrap_or(name)
}

/// Parse a `workspace/symbol` result, keeping only named callables.
///
/// Servers return `SymbolInformation[]` (with `location.range`) or
/// `WorkspaceSymbol[]` (with `location` possibly being `{uri}` only). Both
/// carry `name`, `kind` and a uri, which is all we need.
pub fn parse_workspace_symbols(v: &serde_json::Value) -> Vec<SymbolMatch> {
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|e| {
            let raw_name = e.get("name")?.as_str()?;
            let name = undecorate(raw_name).to_string();
            let kind_n = e.get("kind")?.as_u64()?;
            let kind = match kind_n {
                6 => SymbolKind::METHOD,
                12 => SymbolKind::FUNCTION,
                _ => return None,
            };
            if !is_named_callable(&name, kind) {
                return None;
            }
            let loc = e.get("location")?;
            let uri = Url::parse(loc.get("uri")?.as_str()?).ok()?;
            let start = loc.get("range").and_then(|r| r.get("start"));
            let position = Position {
                line: start
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0) as u32,
                character: start
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0) as u32,
            };
            Some(SymbolMatch {
                name,
                container: e
                    .get("containerName")
                    .and_then(|c| c.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                uri,
                position,
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod workspace_symbol_tests {
    use super::*;
    use serde_json::json;

    fn sample() -> serde_json::Value {
        json!([
          {"name":"send_request","kind":12,
           "containerName":"transport",
           "location":{"uri":"file:///r/src/transport.rs",
                       "range":{"start":{"line":87,"character":3},
                                "end":{"line":87,"character":15}}}},
          {"name":"expect() callback","kind":12,
           "location":{"uri":"file:///r/src/t.ts",
                       "range":{"start":{"line":4,"character":2},
                                "end":{"line":4,"character":9}}}},
          {"name":"Config","kind":23,
           "location":{"uri":"file:///r/src/cfg.rs",
                       "range":{"start":{"line":1,"character":0},
                                "end":{"line":1,"character":6}}}}
        ])
    }

    #[test]
    fn keeps_named_callables() {
        let out = parse_workspace_symbols(&sample());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "send_request");
        assert_eq!(out[0].container.as_deref(), Some("transport"));
        assert_eq!(out[0].position.line, 87);
        assert_eq!(out[0].position.character, 3);
    }

    #[test]
    fn rejects_synthesized_callbacks_and_non_callables() {
        let names: Vec<String> = parse_workspace_symbols(&sample())
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert!(!names.iter().any(|n| n.contains("callback")));
        assert!(!names.contains(&"Config".to_string()));
    }

    #[test]
    fn a_null_or_non_array_result_is_empty_not_a_panic() {
        assert!(parse_workspace_symbols(&serde_json::Value::Null).is_empty());
        assert!(parse_workspace_symbols(&json!({"unexpected": true})).is_empty());
    }

    #[test]
    fn vtsls_decorated_names_are_accepted_and_normalized() {
        let v = json!([
          {"name":"middle()","kind":12,
           "location":{"uri":"file:///r/src/mid.ts",
                       "range":{"start":{"line":2,"character":0},
                                "end":{"line":2,"character":6}}}}
        ]);
        let out = parse_workspace_symbols(&v);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "middle");
    }

    #[test]
    fn a_decorated_name_still_rejects_callback_noise() {
        let v = json!([
          {"name":"expect() callback","kind":12,
           "location":{"uri":"file:///r/src/t.ts",
                       "range":{"start":{"line":4,"character":2},
                                "end":{"line":4,"character":9}}}}
        ]);
        assert!(parse_workspace_symbols(&v).is_empty());
    }

    #[test]
    fn undecorating_is_a_no_op_for_plain_names() {
        let v = json!([
          {"name":"middle","kind":12,
           "location":{"uri":"file:///r/src/mid.rs",
                       "range":{"start":{"line":2,"character":0},
                                "end":{"line":2,"character":6}}}}
        ]);
        let out = parse_workspace_symbols(&v);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "middle");
    }

    #[test]
    fn converts_to_a_named_callable() {
        let m = &parse_workspace_symbols(&sample())[0];
        let c: NamedCallable = m.into();
        assert_eq!(c.name, "send_request");
        assert_eq!(c.position, m.position);
        assert_eq!(c.uri, m.uri);
    }
}
