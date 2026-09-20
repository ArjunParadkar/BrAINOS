//! A body made of several parts.

use brainos_proof::regions::verify_regions;

#[test]
fn every_part_is_governed_by_its_own_limits() {
    let r = verify_regions();
    assert!(
        r.disagreements.is_empty(),
        "{} of {} cases disagreed with the oracle:\n{}",
        r.disagreements.len(),
        r.cases,
        r.disagreements
            .iter()
            .take(6)
            .map(|d| format!(
                "  {} on a {}-part body at {:.1}°: policy said {}, oracle said {} — {}\n",
                d.verb,
                d.parts_on_the_body,
                d.tenths as f64 / 10.0,
                d.policy_said,
                d.oracle_said,
                d.why
            ))
            .collect::<String>()
    );
}

/// A domain where every probe was legal or illegal for every part at
/// once would pass whatever envelope was consulted, and would have
/// passed before the bug was fixed.
#[test]
fn the_domain_actually_discriminates() {
    let r = verify_regions();
    assert!(
        r.is_meaningful(),
        "{} discriminating cases, {} permitted, {} refused — a domain that cannot \
         tell one part's envelope from another's proves nothing",
        r.discriminating,
        r.permitted,
        r.refused
    );
    assert!(r.cases > 100, "only {} cases", r.cases);
    // a good fraction of the domain should be cases where the parts
    // disagree, or the overlap is not doing its job
    assert!(
        r.discriminating * 4 > r.cases,
        "only {} of {} cases discriminate between parts",
        r.discriminating,
        r.cases
    );
}

#[test]
fn it_holds() {
    assert!(verify_regions().holds());
}

/// The domain is worth exactly as much as its ability to fail, so point
/// it at the defect it was written for: a policy holding one envelope
/// for the whole body, which is how every caller built one until a ROS
/// 2 robot turned up with a drive base and a lift.
#[test]
fn the_domain_catches_the_defect_it_was_written_for() {
    use brainos_proof::regions::verify_regions_the_old_way;
    let r = verify_regions_the_old_way();
    assert!(
        !r.holds(),
        "the domain passed a policy with one envelope for the whole body, which \
         means it would not have caught the bug it exists for"
    );
    // and it catches it in both directions: a part refused a motion it
    // should allow, and — the dangerous one — a part allowed a motion
    // some other part's wider envelope permitted
    assert!(
        r.disagreements.iter().any(|d| d.policy_said && !d.oracle_said),
        "the domain found no fail-open case, which is the one that matters"
    );
    assert!(
        r.disagreements.iter().any(|d| !d.policy_said && d.oracle_said),
        "the domain found no fail-closed case"
    );
}
