//! The proposal boundary — where model text stops being text.
//!
//! A Model M reply is untrusted input, exactly like anything else that
//! arrives over a wire. The ONLY thing it can carry besides prose is one
//! typed action PROPOSAL (`say~~verb|target`), and the proposal earns
//! nothing by being parsed: it still faces all eight KIRA stages. This
//! module is deliberately dumb — it extracts structure and nothing else,
//! so there is no clever path for injected text to become authority.

/// Split a Model M reply into (say, action): `say text~~verb|target`.
/// Splits on the FIRST `~~`, so a second injected `~~...` stays inert
/// inside the target string and faces validate/policy like any other byte.
pub fn split_reply(reply: &str) -> (&str, Option<(&str, &str)>) {
    match reply.split_once("~~") {
        Some((say, act)) => {
            let act = act.trim();
            match act.split_once('|') {
                Some((v, t)) if !v.trim().is_empty() => {
                    (say.trim(), Some((v.trim(), t.trim())))
                }
                _ if !act.is_empty() => (say.trim(), Some((act, ""))),
                _ => (say.trim(), None),
            }
        }
        None => (reply.trim(), None),
    }
}
