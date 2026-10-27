//! La scission produit une competition independante, y compris par equipes.

use bp_core::contest::{players::new_player, Contest};
use bp_core::player::{AttrId, PlayerClass};
use bp_core::rounds::splitting::{Criterion, Division, Size, SplitConfig, SplitMode, Splitting};
use bp_core::stage::{Stage, StageKind};
use bp_core::tournament::{user_config::UserConfig, Tournament};

fn enter_split(c: &mut Contest) -> Vec<u32> {
    c.with_schedule(|s, ctx| {
        s.add_kind(ctx, StageKind::Splitting, Some(0));
        s.lock_current(ctx);
        s.enter_next(ctx);
        let split = s
            .stage_mut(1)
            .unwrap()
            .as_any_mut()
            .downcast_mut::<Splitting>()
            .unwrap();
        split.base_mut().name = "Tableau B".into();
        split.base().attendees.clone()
    })
}

fn select(c: &mut Contest, r: u32) {
    c.with_schedule(|s, ctx| {
        s.stage_mut(1)
            .unwrap()
            .as_any_mut()
            .downcast_mut::<Splitting>()
            .unwrap()
            .set_exported(ctx, r, true)
    });
}

#[test]
fn selected_fencers_keep_their_entry_positions_and_source_is_unchanged() {
    let mut c = Contest::new(None, false);
    c.create_default_schedule(true);
    c.name = "Challenge".into();
    for i in 0..5 {
        let mut p = new_player(PlayerClass::Fencer);
        p.set_str(&AttrId::global("name"), &format!("TIREUR{i}"));
        p.set_uint(&AttrId::global("ranking"), i + 1);
        let r = c.add_fencer(p).unwrap();
        c.with_schedule(|s, ctx| {
            s.checkin_mut().unwrap().set_attending(ctx, r, true);
        });
    }
    let refs = enter_split(&mut c);
    select(&mut c, refs[1]);
    select(&mut c, refs[4]);
    let before = c.to_xml();
    let mut t = Tournament::new(UserConfig::new());
    let source = t.manage(c);
    let targets = t.split_current_contest(source).unwrap();
    assert_eq!(targets.len(), 1, "une seule division");
    let target = targets[0];
    assert_ne!(source, target);
    assert_eq!(t.contest_mut(source).unwrap().to_xml(), before);
    let target = t.contest_mut(target).unwrap();
    assert_eq!(target.name, "Challenge - Tableau B");
    assert_eq!(target.label, "Tableau B");
    assert!(target.filename.is_none());
    assert!(target.settings.manual_classification);
    assert!(!target
        .schedule
        .stages()
        .any(|s| s.kind() == StageKind::PoolSupervisor));
    let mut ranks: Vec<_> = target
        .players
        .iter()
        .map(|p| {
            assert!(!p.attr("exported").unwrap().as_bool());
            assert!(p.attr("final_rank").is_none());
            p.attr("ranking").unwrap().as_uint()
        })
        .collect();
    ranks.sort();
    assert_eq!(ranks, vec![2, 5]);
    let xml = target.to_xml();
    let mut loaded = Contest::new(None, false);
    loaded.load_xml_str(&xml).unwrap();
    assert_eq!(loaded.players.len(), 2);
    t.contest_mut(source).unwrap().with_schedule(|s, ctx| {
        s.lock_current(ctx);
        s.enter_next(ctx);
    });
    assert_eq!(
        t.contest(source)
            .unwrap()
            .schedule
            .current()
            .unwrap()
            .base()
            .attendees
            .len(),
        3
    );
}

#[test]
fn split_teams_rebuild_membership_inside_the_target_store() {
    let mut c = Contest::new(None, false);
    c.create_default_schedule(true);
    c.set_team_event(true);
    c.sync_settings();
    for team in ["CLUB A", "CLUB B"] {
        for i in 0..3 {
            let mut p = new_player(PlayerClass::Fencer);
            p.set_str(&AttrId::global("name"), &format!("{team} {i}"));
            p.set_str(&AttrId::global("team"), team);
            p.set_uint(&AttrId::global("ranking"), i + 1);
            let r = c.add_fencer(p).unwrap();
            c.with_schedule(|s, ctx| {
                s.checkin_mut().unwrap().set_attending(ctx, r, true);
            });
        }
    }
    let refs = enter_split(&mut c);
    assert_eq!(refs.len(), 2);
    select(&mut c, refs[1]);
    let source_team = c.players.get(refs[1]).unwrap().clone();
    let mut target = c.duplicate_split(1, 1, None).unwrap();
    let teams: Vec<_> = target
        .players
        .iter()
        .filter(|p| p.class == PlayerClass::Team)
        .collect();
    assert_eq!(teams.len(), 1);
    let team = teams[0];
    assert_eq!(team.name(), source_team.name());
    assert_eq!(team.members.len(), 3);
    assert_eq!(team.attr("ranking").unwrap().as_uint(), 2);
    assert!(team.attending());
    for member in &team.members {
        let player = target.players.get(*member).unwrap();
        assert_eq!(player.class, PlayerClass::Fencer);
        assert_eq!(player.team, Some(team.reference));
        assert!(player.attending());
        assert!(!player.attr("exported").unwrap().as_bool());
    }
    assert_eq!(target.players.len(), 4);
    assert_eq!(c.players.get(refs[1]).unwrap().members, source_team.members);
    let xml = target.to_xml();
    let mut loaded = Contest::new(None, false);
    loaded.load_xml_str(&xml).unwrap();
    assert_eq!(loaded.players.len(), 4);
    let team = loaded
        .players
        .iter()
        .find(|p| p.class == PlayerClass::Team)
        .unwrap();
    assert_eq!(team.members.len(), 3);
    assert!(team
        .members
        .iter()
        .all(|r| loaded.players.get(*r).unwrap().team == Some(team.reference)));
}

/// Dix tireurs classés 1 à 10, nés de 2010 à 2014 (deux par année).
fn ten_fencers() -> (Contest, Vec<u32>) {
    let mut c = Contest::new(None, false);
    c.create_default_schedule(true);
    c.name = "Challenge".into();
    for i in 0..10u32 {
        let mut p = new_player(PlayerClass::Fencer);
        p.set_str(&AttrId::global("name"), &format!("TIREUR{i}"));
        p.set_uint(&AttrId::global("ranking"), i + 1);
        p.set_str(&AttrId::global("birth_date"), &format!("01.03.{}", 2010 + i / 2));
        let r = c.add_fencer(p).unwrap();
        c.with_schedule(|s, ctx| {
            s.checkin_mut().unwrap().set_attending(ctx, r, true);
        });
    }
    let refs = enter_split(&mut c);
    (c, refs)
}

fn configure(c: &mut Contest, config: SplitConfig) {
    c.with_schedule(|s, ctx| {
        s.stage_mut(1).unwrap().as_any_mut().downcast_mut::<Splitting>().unwrap().set_config(ctx, config)
    });
}

fn division(name: &str, size: Option<Size>, criterion: Option<Criterion>) -> Division {
    Division { name: name.into(), size, criterion }
}

/// Noms des tireurs de chaque compétition créée, par libellé.
fn split(c: Contest) -> (Vec<(String, usize)>, usize) {
    let mut t = Tournament::new(UserConfig::new());
    let source = t.manage(c);
    let targets = t.split_current_contest(source).unwrap();
    let made = targets.iter().map(|id| {
        let target = t.contest(*id).unwrap();
        (target.label.clone(), target.players.len())
    }).collect();
    let remaining = t.contest(source).unwrap().schedule.stage(1).unwrap()
        .as_any().downcast_ref::<Splitting>().unwrap().remaining(&t.contest(source).unwrap().players).len();
    (made, remaining)
}

#[test]
fn ranking_divisions_take_the_following_places() {
    let (mut c, _) = ten_fencers();
    configure(&mut c, SplitConfig {
        mode: SplitMode::Ranking,
        keep: Some(Size::Count(4)),
        divisions: vec![division("Division B", Some(Size::Percent(30)), None), division("Division C", None, None)],
    });
    let (made, remaining) = split(c);
    assert_eq!(made, vec![("Division B".into(), 3), ("Division C".into(), 3)]);
    assert_eq!(remaining, 4);
}

#[test]
fn criteria_divisions_use_birth_years_and_keep_the_others() {
    let (mut c, _) = ten_fencers();
    configure(&mut c, SplitConfig {
        mode: SplitMode::Criteria,
        keep: None,
        divisions: vec![
            division("Poussins", None, Some(Criterion::BirthYears { years: vec![2013, 2014] })),
            division("Anciens", None, Some(Criterion::BornBefore { year: 2010 })),
        ],
    });
    let (made, remaining) = split(c);
    assert_eq!(made, vec![("Poussins".into(), 4), ("Anciens".into(), 2)]);
    assert_eq!(remaining, 4, "nés de 2011 à 2012 : ils restent");
}

#[test]
fn rules_and_manual_changes_survive_save_and_load() {
    let (mut c, refs) = ten_fencers();
    let config = SplitConfig {
        mode: SplitMode::Ranking,
        keep: Some(Size::Count(5)),
        divisions: vec![division("Division 2", Some(Size::Count(3)), None), division("Division 3", None, None)],
    };
    configure(&mut c, config.clone());
    // Correction à la main : le premier part en division 2.
    c.with_schedule(|s, ctx| {
        s.stage_mut(1).unwrap().as_any_mut().downcast_mut::<Splitting>().unwrap().set_division(ctx, refs[0], 2)
    });
    let mut loaded = Contest::new(None, false);
    loaded.load_xml_str(&c.to_xml()).unwrap();
    let s = loaded.schedule.stage(1).unwrap().as_any().downcast_ref::<Splitting>().unwrap();
    assert_eq!(s.config, config);
    assert_eq!(s.division_of(&loaded.players, refs[0]), 2);
    assert_eq!(s.divisions_in_use(&loaded.players), vec![1, 2]);
    assert_eq!(s.remaining(&loaded.players).len(), 4);
}

#[test]
fn greg_label_prepares_two_named_divisions() {
    let (mut c, _) = ten_fencers();
    configure(&mut c, SplitConfig {
        mode: SplitMode::Ranking,
        keep: Some(Size::Count(2)),
        divisions: vec![division("A", Some(Size::Count(5)), None)],
    });
    c.with_schedule(|s, ctx| {
        s.stage_mut(1).unwrap().as_any_mut().downcast_mut::<Splitting>().unwrap().set_label(ctx, "N2")
    });
    let (made, remaining) = split(c);
    assert_eq!(made, vec![("N1".into(), 5), ("N2".into(), 3)], "taille de la première division conservée");
    assert_eq!(remaining, 2);
}

#[test]
fn category_division_sets_the_category_of_the_new_competition() {
    let (mut c, _) = ten_fencers();
    configure(&mut c, SplitConfig {
        mode: SplitMode::Criteria,
        keep: None,
        divisions: vec![
            division("Jeunes", None, Some(Criterion::Category { values: vec!["M13".into(), "M15".into()] })),
            division("Grands", None, Some(Criterion::Category { values: vec!["Seniors".into()] })),
        ],
    });
    let s = c.schedule.stage(1).unwrap().as_any().downcast_ref::<Splitting>().unwrap();
    assert_eq!(s.division_category(1).as_deref(), Some("M15"), "la plus âgée des deux");
    assert_eq!(s.division_category(2).as_deref(), Some("S"));
    let target = c.duplicate_split(1, 1, None).unwrap();
    assert_eq!(target.category.xml_image(), "M15");
    assert_eq!(target.name, "Challenge - Jeunes");
}

#[test]
fn birth_year_reads_common_date_formats() {
    use bp_core::rounds::splitting::birth_year;
    assert_eq!(birth_year(Some("12.03.2014")), Some(2014));
    assert_eq!(birth_year(Some("2014-03-12")), Some(2014));
    assert_eq!(birth_year(Some("")), None);
}

#[test]
fn division_continues_the_formula_after_the_split() {
    let (mut c, _) = ten_fencers();
    // Après la scission : tableau (score 10, 4 qualifiés), puis classement.
    c.with_schedule(|s, _| {
        let base = s.stage_mut(2).unwrap().base_mut();
        base.max_score = Some(10);
        base.nb_qualified = Some(4);
    });
    configure(&mut c, SplitConfig {
        mode: SplitMode::Ranking,
        keep: None,
        divisions: vec![division("Division 1", Some(Size::Count(3)), None), division("Division 2", None, None)],
    });
    let target = c.duplicate_split(1, 1, None).unwrap();
    let kinds: Vec<StageKind> = target.schedule.stages().map(|s| s.kind()).collect();
    assert_eq!(kinds, vec![StageKind::Checkin, StageKind::TableSupervisor, StageKind::GeneralClassification], "pas de poules ajoutées");
    let table = target.schedule.stage(1).unwrap();
    assert_eq!(table.base().max_score, Some(10));
    assert_eq!(table.base().nb_qualified, Some(4));
}
