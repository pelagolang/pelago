//! Naming styles.
//!
//! The compiler accepts any naming style (decision B1). The house style, used by the standard library,
//! the docs and the examples, is `camelCase` for functions, methods, variables and fields. So that code
//! written in another style still finds them, a name that is not found is tried once more in the other
//! style: `is_empty` finds `isEmpty` and `isEmpty` finds `is_empty`. An exact match always wins.

/// `is_empty` as `isEmpty`, or `None` when the name has no inner underscore to fold.
pub fn to_camel(name: &str) -> Option<String> {
    let body = name.trim_matches('_');
    if !body.contains('_') {
        return None;
    }
    let lead = &name[..name.len() - name.trim_start_matches('_').len()];
    let tail = &name[name.trim_end_matches('_').len()..];
    let mut out = String::from(lead);
    for (i, part) in body.split('_').filter(|p| !p.is_empty()).enumerate() {
        if i == 0 {
            out.push_str(part);
        } else {
            let mut cs = part.chars();
            if let Some(c) = cs.next() {
                out.extend(c.to_uppercase());
                out.push_str(cs.as_str());
            }
        }
    }
    out.push_str(tail);
    Some(out)
}

/// `isEmpty` as `is_empty`, or `None` when the name has no capital after its first letter.
pub fn to_snake(name: &str) -> Option<String> {
    let mut chars = name.char_indices();
    chars.next();
    if !chars.any(|(_, c)| c.is_uppercase()) {
        return None;
    }
    let mut out = String::with_capacity(name.len() + 2);
    for (i, c) in name.char_indices() {
        if c.is_uppercase() && i > 0 && !out.ends_with('_') {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    Some(out)
}

/// The same name in the other style, if there is one.
pub fn other_style(name: &str) -> Option<String> {
    to_camel(name).or_else(|| to_snake(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_both_ways() {
        assert_eq!(other_style("is_empty").as_deref(), Some("isEmpty"));
        assert_eq!(other_style("isEmpty").as_deref(), Some("is_empty"));
        assert_eq!(other_style("wrapping_add").as_deref(), Some("wrappingAdd"));
        assert_eq!(other_style("_hidden_fn_").as_deref(), Some("_hiddenFn_"));
        assert_eq!(other_style("len"), None);
        assert_eq!(other_style("_x"), None);
    }
}
