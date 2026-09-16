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
    let mut chars = name.chars();
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
        for n in ["types", "handle_request", "_custom", "$ref", "catchall", "uuid"] {
            assert!(is_named_callable(n, SymbolKind::FUNCTION), "should accept {n}");
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
            assert!(!is_named_callable(n, SymbolKind::FUNCTION), "should reject {n}");
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
        for n in ["validar_configuración", "Añadir", "日本語関数", "переменная"] {
            assert!(is_named_callable(n, SymbolKind::FUNCTION), "should accept {n}");
        }
    }

    #[allow(deprecated)]
    fn sym(name: &str, kind: SymbolKind, line: u32, children: Option<Vec<DocumentSymbol>>) -> DocumentSymbol {
        let r = lsp_types::Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 10 },
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
