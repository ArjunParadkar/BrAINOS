//! Stage 2.3 — the network organ faces the gate like any other limb.
//!
//! KIRA's URL floor is about SHAPE, not reputation: it cannot know whether
//! a host is trustworthy, so it refuses only what is malformed or not
//! addressable. These tests pin that line in both directions — a real
//! address must get through, and the malformed/dangerous shapes must not.

mod common;

use brainos_mind::instance::Instance;
use brainos_mind::kira::{valid_url, Action, Verdict};
use common::{bare_instance, embodied_instance};

fn gate(me: &mut Instance, verb: &str, target: &str) -> Verdict {
    let (v, _) = me.propose(Action::UseLimb {
        verb: String::from(verb),
        target: String::from(target),
    });
    v
}

fn denied_at(v: &Verdict, want: &str) -> bool {
    matches!(v, Verdict::Denied { stage, .. } if *stage == want)
}

fn networked() -> Instance {
    let mut me = embodied_instance();
    me.body.incorporate(brainos_mind::body::Region {
        id: String::from("this-machine/net"),
        class: brainos_mind::body::RegionClass::Network,
        capabilities: vec![String::from("web.get"), String::from("web.save")],
        proprioception: String::from("test network organ"),
    });
    me
}

#[test]
fn real_addresses_pass_the_floor() {
    for u in [
        "http://example.com/",
        "https://example.com/a/b?c=d",
        "example.com",            // schemeless: the limb defaults it to http
        "sub.domain.example.org/path",
    ] {
        assert!(valid_url(u), "'{u}' should be a fetchable address");
    }
    let mut me = networked();
    assert!(matches!(gate(&mut me, "web.get", "http://example.com/"), Verdict::Granted(_)));
    assert!(matches!(
        gate(&mut me, "web.save", "http://example.com/ WORK/PAGE.HTM"),
        Verdict::Granted(_)
    ));
}

#[test]
fn malformed_and_dangerous_shapes_die_at_validate() {
    for u in [
        "",
        "not a url",                       // whitespace
        "http://user:pass@example.com/",   // credentials in the address
        "file:///etc/passwd",              // not a fetchable scheme
        "ftp://example.com/x",
        "javascript:alert(1)",
        "http://localhost",                // no dot: not an addressable host
        "http://exa\nmple.com/",           // header/request splitting attempt
    ] {
        assert!(!valid_url(u), "'{u}' must NOT pass the url floor");
    }
    let mut me = networked();
    for u in ["not a url", "http://user:pass@example.com/", "file:///etc/passwd"] {
        assert!(denied_at(&gate(&mut me, "web.get", u), "validate"), "'{u}' escaped validate");
    }
    // oversize
    let long = format!("http://example.com/{}", "a".repeat(300));
    assert!(!valid_url(&long));
}

#[test]
fn a_save_validates_the_disk_path_too() {
    let mut me = networked();
    for t in [
        "http://example.com/",                    // no destination path
        "http://example.com/ ../escape.txt",      // traversal
        "http://example.com/ c:sneaky.txt",       // drive specifier
    ] {
        assert!(
            denied_at(&gate(&mut me, "web.save", t), "validate"),
            "'web.save|{t}' escaped validate"
        );
    }
}

#[test]
fn without_a_network_organ_the_verb_dies_at_authz() {
    // the honest-refusal case: a body with no NIC advertises no web verb,
    // so even a perfectly-formed address is refused before anything runs
    let mut me = embodied_instance();
    assert!(denied_at(&gate(&mut me, "web.get", "http://example.com/"), "authz"));
    let mut bare = bare_instance();
    assert!(denied_at(&gate(&mut bare, "web.get", "http://example.com/"), "authz"));
}

#[test]
fn a_dormant_organ_still_authorizes_nothing() {
    // firmware offers a NIC but no HTTP client: the region exists so the
    // body map is truthful, but it carries no capability to grant
    let mut me = embodied_instance();
    me.body.incorporate(brainos_mind::body::Region {
        id: String::from("this-machine/net"),
        class: brainos_mind::body::RegionClass::Network,
        capabilities: vec![],
        proprioception: String::from("dormant nic"),
    });
    assert!(denied_at(&gate(&mut me, "web.get", "http://example.com/"), "authz"));
}
