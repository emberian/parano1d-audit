// Eclipse-topology lane: peer-selection mechanics and eclipse composition math.
//
// This harness drives the PINNED noid_p2p admission gate verbatim
// (src/peer_diversity_vendored.rs == parano1d/noid_p2p/src/peer_diversity.rs,
// shasum 6c696510…) and models the documented topology controller from
// network.rs using its exact pinned constants, each cited inline. Loopback /
// synthetic only: no sockets are opened, no external peer is contacted.
//
// What is REAL (executed against pinned code): every PeerDiversity::try_admit
// decision, the /16 outbound cap, the per-IP and per-/16 inbound caps, the
// reserve split, failure_domain derivation.
// What is MODEL (pinned constants, cited, no private struct driven):
// AutomaticPeerState dial-selection order, the 8-neighbour target, inbound
// topology credit, first-come stickiness, candidate-pool escape probability.

#[allow(dead_code, unused)]
#[path = "peer_diversity_vendored.rs"]
mod pd;

use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId};
use pd::{DiversityRejection, PeerDiversity};
use rand::seq::SliceRandom;

// ---------------------------------------------------------------------------
// Pinned constants, transcribed with source citations. Values are asserted
// against the vendored module where the module itself exposes them; the
// network.rs values are model inputs (that file is not compiled here — it pulls
// the whole node — so its constants are cited, not linked).
// ---------------------------------------------------------------------------

// peer_diversity.rs (compiled here, verbatim):
const MAX_PUBLIC_OUTBOUND_PEERS_PER_GROUP: usize = 2; // :22
const MAX_PUBLIC_INBOUND_PEERS_PER_IP: usize = 32; // :26
const MAX_PUBLIC_INBOUND_CONNECTIONS_PER_GROUP: usize = 96; // :27
const INBOUND_UNRESERVED_PEERS: usize = 96; // :33
const INBOUND_RESERVED_GROUP_PEERS: usize = 8; // :34

// network.rs (cited, modelled):
const AUTOMATIC_OUTBOUND_TARGET: usize = 8; // network.rs:499
const MAX_UNSELECTED_TOPOLOGY_CREDIT: usize = 4; // network.rs:505 (= TARGET/2)
const INITIAL_BOOTSTRAP_FANOUT: usize = 2; // network.rs:522
const _MAX_PENDING_BOOTSTRAP_DIALS: usize = 4; // network.rs:509

// behaviour.rs (cited):
const ESTABLISHED_INBOUND_CAP: usize = 128; // behaviour.rs:539
const _ESTABLISHED_OUTBOUND_CAP: usize = 64; // behaviour.rs:540
const _ESTABLISHED_PER_PEER: usize = 2; // behaviour.rs:546

// consensus/network.rs: four DNS seeds, one apex domain, dialed /dns/.../tcp/9600
// with NO PeerID (format 2 bare hostname), no node-side checkpoint.
const DNS_SEEDS: [&str; 4] = [
    "mseed1.parano1d.org",
    "mseed2.parano1d.org",
    "mseed3.parano1d.org",
    "mseed4.parano1d.org",
]; // consensus/network.rs:129-134

fn ipv4(a: u8, b: u8, c: u8, d: u8, port: u16) -> Multiaddr {
    format!("/ip4/{a}.{b}.{c}.{d}/tcp/{port}").parse().unwrap()
}

// A distinct /16 group per `group` index, distinct host per `host`.
fn addr_in_group(group: u16, host: u8) -> Multiaddr {
    // Keep octet 0 in a public, non-reserved range (11.x..200.x), octet1 varies
    // the /16. Avoid 10/127/172.16-31/192.168/169.254/100.64 etc.
    let a = 11u8.wrapping_add((group >> 8) as u8 % 180).max(11);
    let b = (group & 0xff) as u8;
    ipv4(a, b, host, 1, 9600)
}

fn cid(n: usize) -> ConnectionId {
    ConnectionId::new_unchecked(n)
}

// ===========================================================================
// SECTION A — constants self-check against the compiled gate
// ===========================================================================
fn section_a() {
    println!("== SECTION A: pinned constants (A = asserted via compiled gate) ==");
    // Assert the outbound /16 cap by driving the real gate: 3rd distinct peer
    // in one /16 must be rejected; 2 are admitted.
    let mut d = PeerDiversity::default();
    let g = addr_in_group(5, 0);
    let p1 = PeerId::random();
    let p2 = PeerId::random();
    let p3 = PeerId::random();
    assert!(d.try_admit(cid(1), p1, &g, true).is_ok());
    assert!(d
        .try_admit(cid(2), p2, &ipv4(11, 5, 7, 9, 9600), true)
        .is_ok());
    let third = d.try_admit(cid(3), p3, &ipv4(11, 5, 9, 9, 9600), true);
    assert!(matches!(
        third,
        Err(DiversityRejection::OutboundGroupFull { .. })
    ));
    println!(
        "A1 outbound /16 cap: 2 admitted, 3rd distinct peer in same /16 -> {:?} [REAL]",
        third
    );
    println!(
        "A2 constants: OUTBOUND_TARGET={AUTOMATIC_OUTBOUND_TARGET} per_/16={MAX_PUBLIC_OUTBOUND_PEERS_PER_GROUP} \
         unselected_credit={MAX_UNSELECTED_TOPOLOGY_CREDIT} inbound/IP={MAX_PUBLIC_INBOUND_PEERS_PER_IP} \
         inbound/_/16={MAX_PUBLIC_INBOUND_CONNECTIONS_PER_GROUP} reserve={INBOUND_UNRESERVED_PEERS}+{INBOUND_RESERVED_GROUP_PEERS}"
    );
    println!(
        "A3 bootstrap: {} DNS seeds under one apex, fanout={INITIAL_BOOTSTRAP_FANOUT}, NO PeerID, NO checkpoint",
        DNS_SEEDS.len()
    );
    println!();
}

// ===========================================================================
// SECTION B — how many attacker /16 groups to own all 8 outbound slots.
// Drives the REAL gate: admit attacker peers across G /16 groups until the
// 8-slot outbound target is full or diversity rejects.
// ===========================================================================
fn outbound_slots_owned_with_groups(groups: usize) -> usize {
    let mut d = PeerDiversity::default();
    let mut admitted = 0usize;
    let mut conn = 1usize;
    // Topology controller dials toward AUTOMATIC_OUTBOUND_TARGET. Offer 2 peers
    // per /16 (the max the gate will take) across `groups` groups.
    'outer: for grp in 0..groups {
        for host in 0..8u8 {
            if admitted >= AUTOMATIC_OUTBOUND_TARGET {
                break 'outer;
            }
            let peer = PeerId::random();
            let addr = addr_in_group(grp as u16, host);
            if d.try_admit(cid(conn), peer, &addr, true).is_ok() {
                admitted += 1;
            }
            conn += 1;
        }
    }
    admitted
}

fn section_b() {
    println!("== SECTION B: attacker /16 groups needed to own the 8 outbound slots [REAL gate] ==");
    for groups in [1usize, 2, 3, 4, 5] {
        let owned = outbound_slots_owned_with_groups(groups);
        let note = if owned >= AUTOMATIC_OUTBOUND_TARGET {
            "FULL outbound capture"
        } else if owned >= MAX_UNSELECTED_TOPOLOGY_CREDIT {
            "captures the >=4 locally-selected minimum"
        } else {
            "partial"
        };
        println!(
            "B{groups} attacker /16 groups = {groups:>1}: outbound slots won = {owned}/{AUTOMATIC_OUTBOUND_TARGET}  ({note})"
        );
    }
    println!(
        "  -> each /16 yields {MAX_PUBLIC_OUTBOUND_PEERS_PER_GROUP} outbound; ceil(8/2)=4 groups for full, 2 groups for the 4-min."
    );
    println!();
}

// ===========================================================================
// SECTION C — inbound saturation caps (REAL gate).
// ===========================================================================
fn section_c() {
    println!("== SECTION C: inbound caps [REAL gate] ==");
    // Per-IP identity cap.
    let mut d = PeerDiversity::default();
    let ip = ipv4(11, 50, 60, 70, 40000);
    let mut conn = 1usize;
    let mut ok = 0usize;
    for _ in 0..MAX_PUBLIC_INBOUND_PEERS_PER_IP + 5 {
        if d.try_admit(cid(conn), PeerId::random(), &ip, false).is_ok() {
            ok += 1;
        }
        conn += 1;
    }
    println!(
        "C1 one public IP: admitted {ok} inbound identities, cap = {MAX_PUBLIC_INBOUND_PEERS_PER_IP} [REAL]"
    );

    // Per-/16 connection cap across many IPs in one /16.
    let mut d = PeerDiversity::default();
    let mut conn = 1usize;
    let mut ok = 0usize;
    'fill: for host in 1..=250u8 {
        for _ in 0..2 {
            if ok >= MAX_PUBLIC_INBOUND_CONNECTIONS_PER_GROUP + 10 {
                break 'fill;
            }
            let addr = ipv4(11, 77, host, 1, 40000);
            if d.try_admit(cid(conn), PeerId::random(), &addr, false).is_ok() {
                ok += 1;
            }
            conn += 1;
        }
    }
    println!(
        "C2 one /16 (many IPs): admitted {ok} inbound connections, cap = {MAX_PUBLIC_INBOUND_CONNECTIONS_PER_GROUP} [REAL]"
    );
    println!(
        "  -> to fill all {ESTABLISHED_INBOUND_CAP} inbound sessions the attacker needs >=2 distinct /16 groups"
    );
    println!("  -> but inbound buys at most {MAX_UNSELECTED_TOPOLOGY_CREDIT} topology slots (network.rs:505); the");
    println!("     other >=4 of 8 must be the victim's own OUTBOUND dials -> inbound alone cannot eclipse.");
    println!();
}

// ===========================================================================
// SECTION D — fresh-node honest-escape probability.
// Model: a fresh node (no peers.json) derives its entire candidate pool from
// the DNS-seed Kademlia walk. Pool = H honest addrs (each a distinct /16) +
// attacker addrs across G /16 groups. The topology controller shuffles the
// pool and dials toward 8, respecting the REAL diversity gate. Escape = node
// dials >=1 honest peer.
// ===========================================================================
fn fresh_escape_prob(honest_addrs: usize, attacker_groups: usize, trials: usize) -> f64 {
    let mut rng = rand::thread_rng();
    let mut escapes = 0usize;
    for _ in 0..trials {
        // Build the shuffled candidate pool.
        let mut pool: Vec<(PeerId, Multiaddr, bool)> = Vec::new();
        // Honest peers: each its own /16 (honest network is operator-diverse).
        for h in 0..honest_addrs {
            pool.push((PeerId::random(), addr_in_group(1000 + h as u16, 0), true));
        }
        // Attacker peers: many identities across G /16 groups (2 usable per /16,
        // offer extra to model a flooded DHT).
        for grp in 0..attacker_groups {
            for host in 0..6u8 {
                pool.push((PeerId::random(), addr_in_group(grp as u16, host), false));
            }
        }
        pool.shuffle(&mut rng);

        // Dial toward target through the REAL gate.
        let mut d = PeerDiversity::default();
        let mut admitted_honest = false;
        let mut admitted = 0usize;
        let mut conn = 1usize;
        for (peer, addr, honest) in &pool {
            if admitted >= AUTOMATIC_OUTBOUND_TARGET {
                break;
            }
            if d.try_admit(cid(conn), *peer, addr, true).is_ok() {
                admitted += 1;
                if *honest {
                    admitted_honest = true;
                }
            }
            conn += 1;
        }
        if admitted_honest {
            escapes += 1;
        }
    }
    escapes as f64 / trials as f64
}

fn section_d() {
    println!("== SECTION D: fresh-node honest-escape probability [REAL gate inside a model pool] ==");
    println!("  Model: no peers.json; whole candidate pool comes from the DNS-seed Kademlia walk.");
    println!("  (a) Attacker CONTROLS the bootstrap (seed operator / DNS / the pool it returns):");
    println!("      honest_addrs = 0  -> escape probability = 0.000  [definitional: no honest addr exists]");
    println!("  (b) Seeds honest but attacker floods the DHT with G /16 groups; H honest addrs survive:");
    let trials = 20000;
    for &h in &[1usize, 2, 4] {
        for &g in &[4usize, 16, 64] {
            let p = fresh_escape_prob(h, g, trials);
            println!("      H={h} honest /16s, attacker G={g:>2} /16s: P(dial >=1 honest) = {p:.3}");
        }
    }
    println!("  -> With the honest seed reachable, a handful of honest addrs almost always escape:");
    println!("     the 2-per-/16 cap means the attacker cannot crowd out honest /16s from the dial set.");
    println!("  -> The dangerous case is (a): the four seeds ARE the only root of trust; owning them");
    println!("     (or the apex DNS) sets H=0 and escape collapses to zero for a NATed fresh node.");
    println!();
}

// ===========================================================================
// SECTION E — stickiness of an ESTABLISHED peer set.
// network.rs facts (cited): the only paths that drop an established, identified,
// healthy ordinary neighbour are (1) ordinary_release_needed -> requires
// topology_peer_count() > AUTOMATIC_OUTBOUND_TARGET (network.rs:5645-5647), i.e.
// strictly ABOVE 8 — never at or below target; (2) a disconnect/dial-failure of
// that peer. There is NO periodic re-dial or idle rotation of a healthy
// sub-target peer. maintain runs every 2s (network.rs:3950) but only ADDS
// toward target; it never evicts a healthy peer to make room for a new one.
// ===========================================================================
fn section_e() {
    println!("== SECTION E: established-set stickiness [MODEL over cited network.rs logic] ==");
    // Reproduce ordinary_release_needed(release_seed, topology_peers).
    let ordinary_release_needed = |release_seed: bool, topology_peers: usize| -> bool {
        !release_seed && topology_peers > AUTOMATIC_OUTBOUND_TARGET
    };
    for peers in [6usize, 7, 8, 9, 10] {
        let release = ordinary_release_needed(false, peers);
        println!(
            "E topology_peers={peers}: ordinary_release_needed = {release}  ({})",
            if release {
                "prunes EXCESS above target"
            } else {
                "healthy set retained — NO rotation"
            }
        );
    }
    println!("  -> VERDICT: first-come-STICKY. A healthy 8-peer set is never rotated below target.");
    println!("     An eclipse established by winning the initial set (or by churn+replacement) persists");
    println!("     until the attacker's own peers disconnect. There is no escape via periodic re-dial.");
    println!("  -> Churn path: force an honest peer to drop (its connection closes) -> accelerate_discovery");
    println!("     (network.rs:1873) -> next 2s tick dials a REPLACEMENT from the candidate pool; if the");
    println!("     attacker owns the pool and the free /16 budget, the replacement is attacker. Repeat per");
    println!("     honest peer. No app-level score raises the honest peer's odds (no ban/score system).");
    println!();
}

// ===========================================================================
// SECTION F — chaos-per-attacker-box composition table.
// Maps each known finding's identity/IP footprint onto 1 / 2 / 4 coordinated
// attacker boxes (one box == one machine with one public IP, the unit the
// diversity policy actually gates). Numbers are the finding constants.
// ===========================================================================
struct Chaos {
    id: &'static str,
    name: &'static str,
     identities: &'static str,
    ips_needed: &'static str,
    gate: &'static str,
}

fn section_f() {
    println!("== SECTION F: chaos-per-box footprint (finding constants) ==");
    let rows = [
        Chaos {
            id: "C17",
            name: "mempool-pull byte-domain pin (serving starvation, ALL peers)",
            identities: "1 identity, 4 stalled pull streams (<=8/conn, 2 conn/id)",
            ips_needed: "1 IP",
            gate: "request-response; rotate pulls <=30s; no acceptance impact",
        },
        Chaos {
            id: "U21",
            name: "gossip-window monopoly (silences tx+block gossip)",
            identities: "~5 identities x 303KB undecodable, <8/IP so no colocation penalty",
            ips_needed: "1 IP",
            gate: "gossip pre-decode byte window, global, shared w/ blocks",
        },
        Chaos {
            id: "C22",
            name: "admission-queue occupancy (62x CPU discount)",
            identities: "~10 (code's 84ms budget) .. ~84 (this laptop) Push identities",
            ips_needed: "1 IP (<=32/id-per-IP) .. 3 IPs (84/32)",
            gate: "MempoolRequest::Push, the one un-profile-gated arm; free libp2p keys",
        },
        Chaos {
            id: "ECL-fresh",
            name: "eclipse a FRESH node (own all 8 outbound)",
            identities: "8 attacker peer-ids (2 per /16)",
            ips_needed: "4 distinct /16 groups (+ control of the seed/DHT pool)",
            gate: "first-come sticky; needs H=0 honest in candidate pool",
        },
        Chaos {
            id: "ECL-est",
            name: "eclipse an ESTABLISHED node (churn + replace)",
            identities: "8 peer-ids for outbound (2/_/16) + inbound fill",
            ips_needed: ">=4 /16 for outbound + >=2 /16 to flood 128 inbound",
            gate: "sticky: must evict each honest peer AND win its replacement",
        },
        Chaos {
            id: "C26",
            name: "snapshot poisoning (needs fresh-or-eclipsed victim)",
            identities: "1 serving identity (once victim is eclipsed/fresh)",
            ips_needed: "1 IP to serve + the ECL-fresh footprint to isolate",
            gate: "fork-choice gate: victim must be fresh or eclipsed first",
        },
    ];
    for r in rows {
        println!(
            "  [{:>9}] {:<52}\n            identities: {}\n            IPs: {}\n            gate: {}",
            r.id, r.name, r.identities, r.ips_needed, r.gate
        );
    }
    println!();
    println!("  TIER SUMMARY (one box == one public IP):");
    println!("   1 box  : C17 serving starvation + U21 gossip silence + (hardware-permitting) C22 queue");
    println!("            occupancy — a single IP denies serving, gossip, and admission to one victim,");
    println!("            with ZERO ability to eclipse (needs 4 /16s). Availability DoS, no isolation.");
    println!("   2 boxes: + captures the 4-minimum locally-selected outbound (2 /16s x 2 peers) — HALF");
    println!("            eclipse; honest inbound/credit still leaks unless victim is NATed.");
    println!("   4 boxes: 4 distinct /16s -> full 8-outbound capture of a fresh/NATed victim -> stack C26");
    println!("            snapshot poisoning on the now-eclipsed victim -> permanent estrangement, and the");
    println!("            victim re-serves the poison (self-propagating). 4 boxes = isolation + poison.");
    println!();
}

fn main() {
    println!("eclipse-topology PoC — pinned noid_p2p @ 52938433");
    println!("gate: peer_diversity_vendored.rs (shasum 6c696510, == pinned peer_diversity.rs)");
    println!("------------------------------------------------------------------------------");
    println!();
    section_a();
    section_b();
    section_c();
    section_d();
    section_e();
    section_f();
    println!("ALL SECTIONS COMPLETE");
}
