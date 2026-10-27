//! Lobby (série) dans une compétition : composition à l'entrée, arrivées,
//! enregistrement puis relecture du fichier, qualifiés de la phase suivante.

use bp_core::contest::{players::new_player, Contest};
use bp_core::player::{AttrId, PlayerClass};
use bp_core::rounds::lobby::{Lobby, LobbySettings, ABANDON};
use bp_core::stage::{Stage, StageKind};

fn contest_with_lobby(n: u32) -> Contest {
    let mut c = Contest::new(None, false);
    c.create_default_schedule(true);
    c.name = "Course".into();
    for i in 0..n {
        let mut p = new_player(PlayerClass::Fencer);
        p.set_str(&AttrId::global("name"), &format!("PILOTE{i}"));
        p.set_uint(&AttrId::global("ranking"), i + 1);
        let r = c.add_fencer(p).unwrap();
        c.with_schedule(|s, ctx| s.checkin_mut().unwrap().set_attending(ctx, r, true));
    }
    c.with_schedule(|s, ctx| {
        let at = s.add_kind(ctx, StageKind::Lobby, Some(0));
        let l = s.stage_mut(at).unwrap().as_any_mut().downcast_mut::<Lobby>().unwrap();
        l.settings = LobbySettings { lobbies: 2, races: 2, swapping: Vec::new(), per_lobby: Some(2), ..Default::default() };
        s.lock_current(ctx);
        s.enter_next(ctx);
    });
    c
}

fn lobby(c: &Contest) -> &Lobby {
    c.schedule.stage(1).unwrap().as_any().downcast_ref::<Lobby>().unwrap()
}

#[test]
fn lobby_results_survive_saving_and_qualify_per_lobby() {
    let mut c = contest_with_lobby(8);
    assert_eq!(c.schedule.current().unwrap().kind(), StageKind::Lobby);
    assert_eq!(lobby(&c).lobbies().len(), 2);
    // Deux manches dans chaque lobby ; un abandon.
    c.with_schedule(|s, _| {
        let l = s.stage_mut(1).unwrap().as_any_mut().downcast_mut::<Lobby>().unwrap();
        for k in 0..2 {
            let order = l.lobbies()[k].clone();
            for race in 0..2 {
                for (place, r) in order.iter().enumerate() {
                    let finish = if k == 1 && race == 1 && place == 3 { ABANDON } else { place as i32 + 1 };
                    l.set_finish(k, race, *r, Some(finish));
                }
            }
        }
    });
    // Export HTML (résultats complets) : chaque lobby et ses manches.
    let html = c.dump_to_html();
    assert!(html.contains("Lobby 1") && html.contains("Lobby 2") && html.contains("M2"), "lobbys dans l'export");
    let xml = c.to_xml();
    let mut copy = Contest::new(None, false);
    copy.load_xml_str(&xml).unwrap();
    let (a, b) = (lobby(&c), lobby(&copy));
    assert_eq!(a.lobbies(), b.lobbies());
    for k in 0..2 {
        for r in &a.lobbies()[k] {
            assert_eq!(a.standing(*r), b.standing(*r));
        }
    }
    // Deux qualifiés par lobby : les deux premiers de chaque lobby.
    copy.with_schedule(|s, ctx| {
        s.lock_current(ctx);
        s.enter_next(ctx);
    });
    let next = copy.schedule.current().unwrap();
    assert_eq!(next.base().attendees.len(), 4);
}
