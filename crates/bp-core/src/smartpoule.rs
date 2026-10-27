//! SmartPoule : données pour l'application des arbitres (téléphone).
//!
//! L'application d'origine n'existe plus ; celle-ci dialogue en JSON avec
//! BellePoule (voir `smartpoule-android/` et `bellepoule/src/network.rs`).
//!
//! Un match à arbitrer est désigné par un jeton :
//! - poule : `p~<compétition>~<tour>~<n° de poule>` ;
//! - tableau : `t~<compétition>~<phase>~<suite>~<chemin du match>` ;
//! - lobby (série) : `l~<compétition>~<phase>~<n° du lobby>` (saisie : la
//!   manche en `bout`, le tireur et sa place dans `values`).
//!
//! Un arbitre voit les poules et les matchs auxquels il est affecté (pour une
//! poule sur plusieurs pistes : les matchs de sa piste) ; l'administrateur
//! (`referee = None`) voit tout.

use serde_json::{json, Value};

use crate::bout::Match;
use crate::contest::Contest;
use crate::player::{PlayerRef, PlayerStore};
use crate::rounds::pool::PoolAllocatorStage;
use crate::rounds::table::TableSupervisor;

/// Jeton d'une poule (QR code de sa feuille, matchs du téléphone).
pub fn pool_token(contest: u32, round: u32, number: u32) -> String {
    format!("p~{contest}~{round}~{number}")
}

/// Jeton d'un match de tableau (`chemin` : `TableSet::match_path`).
pub fn match_token(contest: u32, stage: u32, set: &str, path: &str) -> String {
    format!("t~{contest}~{stage}~{set}~{path}")
}

/// Code PIN (4 chiffres) d'un arbitre, tiré de sa clé personnelle : il
/// change avec elle et n'a pas à être enregistré à part. Il confirme
/// l'arbitre qui scanne le QR code d'une feuille.
pub fn referee_pin(key: &str) -> String {
    // FNV-1a 64 bits : stable d'une plateforme à l'autre.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in "managecomp-pin|".bytes().chain(key.bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:04}", h % 10_000)
}

/// Nouvelle clé personnelle d'arbitre (32 caractères, QR code personnel).
pub fn new_referee_key() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..32).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

/// Clé personnelle valide (32 caractères).
pub fn is_referee_key(key: &str) -> bool {
    key.len() == 32
}

fn identity(players: &PlayerStore, r: PlayerRef) -> (String, String) {
    players
        .get(r)
        .map(|p| {
            let name = format!("{} {}", p.name(), p.str_attr("first_name").unwrap_or_default()).trim().to_string();
            let club = p.str_attr("club").unwrap_or_default().to_string();
            (name, club)
        })
        .unwrap_or_default()
}

/// `<Match><Tireur REF Score Statut/>…</Match>` à partir des saisies
/// (`V`, `W3`/`V3`, `4`, vide), comme le message `SmartPoule::Score`.
fn match_xml(m: &Match, values: &[String; 2]) -> Option<String> {
    let mut candidate = m.clone();
    let mut xml = String::from("<Match>");
    for (k, value) in values.iter().enumerate() {
        let fencer = m.opponent(k)?;
        let v = value.trim().to_ascii_uppercase();
        let (score, victory) = if v == "V" {
            (m.max_score, true)
        } else if let Some(n) = v.strip_prefix('V').or_else(|| v.strip_prefix('W')) {
            (n.parse::<u32>().ok()?, true)
        } else if v.is_empty() {
            return None;
        } else {
            (v.parse::<u32>().ok()?, false)
        };
        candidate.set_score(fencer, score, victory);
        xml.push_str(&format!(
            "<Tireur REF=\"{fencer}\" Score=\"{score}\" Statut=\"{}\"/>",
            if victory { "V" } else { "D" }
        ));
    }
    if candidate.has_error() { return None; }
    xml.push_str("</Match>");
    Some(xml)
}

impl Contest {
    /// Chaque arbitre reçoit une clé personnelle (QR code personnel, dont
    /// découle son PIN) s'il n'en a pas. Vrai si une clé a été créée.
    pub fn ensure_referee_keys(&mut self) -> bool {
        let id = crate::player::AttrId::global("password");
        let mut changed = false;
        for r in self.referees.refs().to_vec() {
            if let Some(p) = self.referees.get_mut(r) {
                if !p.str_attr("password").is_some_and(is_referee_key) {
                    p.set_str(&id, &new_referee_key());
                    changed = true;
                }
            }
        }
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// Nouvelle clé (donc nouveau PIN et nouveau QR code personnel) d'un arbitre.
    pub fn renew_referee_key(&mut self, reference: u32) -> Option<String> {
        let key = new_referee_key();
        self.referees.get_mut(reference)?.set_str(&crate::player::AttrId::global("password"), &key);
        self.dirty = true;
        Some(key)
    }

    /// Jeton de la feuille d'une poule du tour de poules `index` (phase des poules).
    pub fn pool_sheet_token(&self, index: usize, number: u32) -> Option<String> {
        let stage = self.schedule.stage(index)?;
        let round = stage.as_any().downcast_ref::<crate::rounds::pool::PoolSupervisorStage>()?.round();
        Some(pool_token(self.net_id, round.net_id(), number))
    }

    fn title(&self) -> String {
        format!("{} · {} · {} · {}", self.name, self.weapon.label(), self.gender_label(), self.category.display_image())
    }

    /// Poules et matchs à arbitrer par `referee` (tous pour l'administrateur).
    pub fn smartpoule_jobs(&self, referee: Option<u32>) -> Vec<Value> {
        let mut out = Vec::new();
        let current = self.schedule.current_index();
        for i in 0..self.schedule.len().min(current + 1) {
            let Some(stage) = self.schedule.stage(i) else { continue };
            // Poules du tour en cours (superviseur non verrouillé)
            if let Some(a) = stage.as_any().downcast_ref::<PoolAllocatorStage>() {
                if self.schedule.stage(i + 1).map(|s| s.base().locked).unwrap_or(true) {
                    continue;
                }
                let round = a.round();
                // Les matchs n'existent qu'une fois la composition des poules validée.
                if !round.allocator_locked {
                    continue;
                }
                for pool in round.pools() {
                    let mine = match referee {
                        None => true,
                        Some(r) => {
                            pool.referees().contains(&r) || pool.match_assignments_with_clubs(&self.players, self.referees.store()).iter().any(|m| m.referee == Some(r))
                                || pool.group().is_some_and(|g| crate::group_sheets::group_dispatch(&round, g, &self.players, self.referees.store()).iter().any(|b| b.referee == Some(r) && round.pools().get(b.pool).map(|p| p.number()) == Some(pool.number())))
                        }
                    };
                    if !mine || pool.nb_players() == 0 {
                        continue;
                    }
                    out.push(json!({
                        "token": pool_token(self.net_id, round.net_id(), pool.number()),
                        "kind": "pool",
                        "color": self.color,
                        "competition": self.title(),
                        "name": pool.name(),
                        "piste": pool.piste_label().unwrap_or_default(),
                        "time": pool.start_time_image().unwrap_or_default(),
                        "over": pool.is_over(),
                        "fencers": pool.nb_players(),
                        "referees": pool.referees().iter().copied().chain(pool.match_assignments_with_clubs(&self.players, self.referees.store()).iter().filter_map(|m| m.referee)).collect::<Vec<_>>(),
                    }));
                }
            }
            // Lobbys (série) de la phase en cours
            if let Some(l) = stage.as_any().downcast_ref::<crate::rounds::lobby::Lobby>() {
                if !stage.base().locked && i == current {
                    for (k, members) in l.lobbies().iter().enumerate() {
                        let road = l.roadmap(k).cloned().unwrap_or_default();
                        if let Some(r) = referee {
                            if !road.referees.contains(&r) {
                                continue;
                            }
                        }
                        out.push(json!({
                            "token": lobby_token(self.net_id, stage.base().net_id, k),
                            "kind": "lobby",
                            "color": self.color,
                            "competition": self.title(),
                            "name": format!("Lobby {}", k + 1),
                            "piste": road.pistes.iter().map(|p| format!("{p:02}")).collect::<Vec<_>>().join(", "),
                            "time": road.time,
                            "over": l.lobby_over(k),
                            "fencers": members.len(),
                            "summary": format!("{} manche{}", l.settings.races.max(1), if l.settings.races > 1 { "s" } else { "" }),
                            "referees": road.referees,
                        }));
                    }
                }
            }
            // Matchs de tableau prêts
            if let Some(sup) = stage.as_any().downcast_ref::<TableSupervisor>() {
                if stage.base().locked {
                    continue;
                }
                for set in sup.table_sets().iter().filter(|s| s.is_visible()) {
                    for (node, n) in set.nodes().iter().enumerate() {
                        let Some(m) = n.bout.as_ref() else { continue };
                        if m.opponent(0).is_none() || m.opponent(1).is_none() || m.number == 0 {
                            continue;
                        }
                        if let Some(r) = referee {
                            if !m.referees.contains(&r) {
                                continue;
                            }
                        }
                        let Some(path) = set.match_path(node) else { continue };
                        let (a, _) = identity(&self.players, m.opponent(0).unwrap());
                        let (b, _) = identity(&self.players, m.opponent(1).unwrap());
                        out.push(json!({
                            "token": match_token(self.net_id, stage.base().net_id, set.id(), &path),
                            "kind": "match",
                            "competition": self.title(),
                            "name": format!("{} — {}", set.name(), m.name),
                            "piste": if m.piste != 0 { format!("{:02}", m.piste) } else { String::new() },
                            "time": m.start_time.map(|t| t.0.format("%H:%M").to_string()).unwrap_or_default(),
                            "over": m.is_over(),
                            "fencers": 2,
                            "summary": format!("{a} / {b}"),
                            "referees": m.referees.clone(),
                        }));
                    }
                }
            }
        }
        out
    }

    /// Feuille d'un match à arbitrer (poule complète, ou match de tableau).
    pub fn smartpoule_sheet(&self, token: &str, referee: Option<u32>) -> Option<Value> {
        let parts: Vec<&str> = token.split('~').collect();
        if parts.get(1)?.parse::<u32>().ok()? != self.net_id {
            return None;
        }
        let stage_id: u32 = parts.get(2)?.parse().ok()?;
        match *parts.first()? {
            "p" => {
                let number: u32 = parts.get(3)?.parse().ok()?;
                for stage in self.schedule.stages() {
                    let Some(a) = stage.as_any().downcast_ref::<PoolAllocatorStage>() else { continue };
                    let round = a.round();
                    if round.net_id() != stage_id {
                        continue;
                    }
                    let pool_index = round.pools().iter().position(|p| p.number() == number)?;
                    let pool = &round.pools()[pool_index];
                    let sheet = pool.sheet(&self.players, self.referees.store(), false);
                    // Entraînement des jeunes : numéros, pistes et arbitres du groupe entier.
                    let group_bouts: Option<Vec<crate::group_sheets::GroupBout>> = pool.group().map(|g| {
                        crate::group_sheets::group_dispatch(&round, g, &self.players, self.referees.store()).into_iter().filter(|b| b.pool == pool_index).collect()
                    });
                    let group_bout = |m: usize| group_bouts.as_ref().and_then(|l| l.iter().find(|b| b.match_index == m));
                    let referee_name = |r: u32| self.referees.store().get(r).map(|p| format!("{} {}", p.name(), p.str_attr("first_name").unwrap_or_default()).trim().to_string()).unwrap_or_default();
                    let mut matches: Vec<_> = sheet.matches.iter().collect();
                    if group_bouts.is_some() {
                        matches.sort_by_key(|m| group_bout(m.match_index).map(|b| b.number).unwrap_or(usize::MAX));
                    }
                    let bouts: Vec<Value> = matches
                        .into_iter()
                        .filter(|m| match (referee, group_bout(m.match_index), &m.assignment) {
                            (Some(r), Some(b), _) if b.referee.is_some() => b.referee == Some(r),
                            (Some(r), None, Some(a)) if pool.strips().len() > 1 || pool.referees().len() > 1 => a.referee == Some(r),
                            _ => true,
                        })
                        .map(|m| {
                            let bout = group_bout(m.match_index);
                            json!({
                                "index": m.match_index,
                                "name": bout.map(|b| format!("M{}", b.number)).unwrap_or_else(|| m.name.clone()),
                                "referee": bout.map(|b| b.referee).unwrap_or_else(|| m.assignment.as_ref().and_then(|a| a.referee)),
                                // Piste du match (écran LivePoule de la piste).
                                "piste": bout.map(|b| b.piste)
                                    .or_else(|| m.assignment.as_ref().map(|a| a.piste))
                                    .or_else(|| pool.strips().first().copied())
                                    .filter(|p| *p != 0),
                                "resource": match bout {
                                    // Piste et arbitre du match, quand ils sont connus.
                                    Some(b) => {
                                        let parts: Vec<String> = [
                                            (b.piste != 0).then(|| format!("Piste {:02}", b.piste)),
                                            b.referee.map(referee_name).filter(|n| !n.is_empty()),
                                        ]
                                        .into_iter()
                                        .flatten()
                                        .collect();
                                        (!parts.is_empty()).then(|| parts.join("\n"))
                                    }
                                    None => m.resource.clone(),
                                },
                                "fencers": m.opponents.iter().flatten().map(|f| json!({
                                    "reference": f.player,
                                    "position": f.position,
                                    "name": format!("{} {}", f.identity.name, f.identity.first_name).trim().to_string(),
                                    "club": f.identity.club,
                                    "score": f.score,
                                    "members": self.team_members(f.player),
                                })).collect::<Vec<_>>(),
                                "relay_log": pool.matches().get(m.match_index).and_then(|x| x.bout.relay.as_ref()),
                                "over": pool.matches().get(m.match_index).map(|x| x.bout.is_over()).unwrap_or(false),
                                "officiating": pool.matches().get(m.match_index).and_then(|x| x.bout.officiating.as_ref()),
                            })
                        })
                        .collect();
                    return Some(json!({
                        "token": token,
                        "kind": "pool",
                        "color": self.color,
                        "competition": self.title(),
                        "name": pool.name(),
                        "piste": pool.piste_label().unwrap_or_default(),
                        "max_score": pool.max_score(),
                        // `round` est déjà verrouillé : le getter de la phase
                        // reprendrait le même Mutex et bloquerait la synchro.
                        "timing": round.bout_timing,
                        // Rencontres par équipes : format des relais.
                        "relay": self.settings.team_event.then(|| { let mut f = round.bout_timing.relay; f.team_size = self.settings.team_size; f.substitutes = self.settings.substitutes; f }),
                        "fencers": sheet.rows.iter().map(|r| json!({
                            "reference": r.player,
                            "position": r.position,
                            "name": format!("{} {}", r.identity.name, r.identity.first_name).trim().to_string(),
                            "club": r.identity.club,
                            "cells": r.cells.iter().map(|c| if c.grey { Value::Null } else { json!(c.text) }).collect::<Vec<_>>(),
                            "victories": r.dashboard.as_ref().map(|d| d.victories.clone()),
                            "indice": r.dashboard.as_ref().map(|d| d.indice.clone()),
                            "rank": r.dashboard.as_ref().map(|d| d.rank.clone()),
                        })).collect::<Vec<_>>(),
                        "bouts": bouts,
                        "over": pool.is_over(),
                    }));
                }
                None
            }
            "t" => {
                let (set_id, path) = (parts.get(3)?, parts.get(4)?);
                for stage in self.schedule.stages() {
                    let Some(sup) = stage.as_any().downcast_ref::<TableSupervisor>() else { continue };
                    if stage.base().net_id != stage_id {
                        continue;
                    }
                    let set = sup.table_set(sup.table_set_by_id(set_id)?);
                    let node = set.node_from_path(path)?;
                    let m = set.bout(node)?;
                    let fencers: Vec<Value> = (0..2)
                        .filter_map(|k| {
                            let f = m.opponent(k)?;
                            let (name, club) = identity(&self.players, f);
                            Some(json!({ "reference": f, "position": k + 1, "name": name, "club": club, "score": m.score_at(k).image(m.max_score), "members": self.team_members(f) }))
                        })
                        .collect();
                    return Some(json!({
                        "token": token,
                        "kind": "match",
                        "color": self.color,
                        "competition": self.title(),
                        "name": format!("{} — {}", set.name(), m.name),
                        "piste": if m.piste != 0 { format!("{:02}", m.piste) } else { String::new() },
                        "max_score": m.max_score,
                        "timing": stage.bout_timing(),
                        "relay": self.settings.team_event.then(|| { let mut f = stage.bout_timing().relay; f.team_size = self.settings.team_size; f.substitutes = self.settings.substitutes; f }),
                        "bouts": [{ "index": 0, "name": m.name, "piste": Some(m.piste).filter(|p| *p != 0), "fencers": fencers, "over": m.is_over(), "officiating": m.officiating, "relay_log": m.relay }],
                        "over": m.is_over(),
                    }));
                }
                None
            }
            "l" => {
                let k: usize = parts.get(3)?.parse().ok()?;
                let stage = self.schedule.stages().find(|s| s.base().net_id == stage_id)?;
                let l = stage.as_any().downcast_ref::<crate::rounds::lobby::Lobby>()?;
                let members = l.lobbies().get(k)?;
                let races = l.settings.races.max(1) as usize;
                let ranking = l.lobby_ranking(k);
                let road = l.roadmap(k).cloned().unwrap_or_default();
                let referee_name = |r: u32| self.referees.store().get(r).map(|p| format!("{} {}", p.name(), p.str_attr("first_name").unwrap_or_default()).trim().to_string()).unwrap_or_default();
                Some(json!({
                    "token": token,
                    "kind": "lobby",
                    "name": format!("Lobby {}", k + 1),
                    "competition": self.title(),
                    "color": self.color,
                    "piste": road.pistes.iter().map(|p| format!("{p:02}")).collect::<Vec<_>>().join(", "),
                    "time": road.time,
                    "referees": road.referees.iter().map(|r| referee_name(*r)).collect::<Vec<_>>(),
                    "races": races,
                    "locked": stage.base().locked,
                    "over": l.lobby_over(k),
                    "fencers": members.iter().map(|r| {
                        let (name, club) = identity(&self.players, *r);
                        let s = l.standing(*r);
                        json!({
                            "reference": r,
                            "name": name,
                            "club": club,
                            "finishes": (0..races).map(|race| match l.finish(k, race, *r) {
                                Some(crate::rounds::lobby::ABANDON) => json!("A"),
                                Some(p) => json!(p),
                                None => Value::Null,
                            }).collect::<Vec<_>>(),
                            "points": s.points,
                            "place": ranking.iter().find(|(x, _, _)| x == r).map(|(_, p, _)| *p),
                            "raced": s.races > 0,
                        })
                    }).collect::<Vec<_>>(),
                    "bouts": [],
                }))
            }
            _ => None,
        }
    }

    /// Score d'un match saisi sur le téléphone (`values` : tireur du haut,
    /// tireur du bas). Renvoie une erreur lisible si la saisie est refusée.
    pub fn smartpoule_apply_score(&mut self, token: &str, bout: usize, values: [String; 2]) -> Result<(), String> {
        if token.split('~').nth(1).and_then(|s| s.parse::<u32>().ok()) != Some(self.net_id) { return Err("compétition invalide".into()); }
        let parts: Vec<&str> = token.split('~').collect();
        let stage_id: u32 = parts.get(2).and_then(|s| s.parse().ok()).ok_or("jeton invalide")?;
        match parts.first().copied() {
            Some("p") => {
                let number: u32 = parts.get(3).and_then(|s| s.parse().ok()).ok_or("jeton invalide")?;
                for i in 0..self.schedule.len() {
                    let Some(stage) = self.schedule.stage(i) else { continue };
                    let Some(a) = stage.as_any().downcast_ref::<PoolAllocatorStage>() else { continue };
                    let shared = a.shared_round();
                    if shared.lock().map(|r| r.net_id()).unwrap_or(0) != stage_id {
                        continue;
                    }
                    if self.schedule.stage(i + 1).map(|s| s.base().locked).unwrap_or(true) {
                        return Err("le tour de poules est terminé".into());
                    }
                    let mut round = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let players = &mut self.players;
                    let pool = round.pools_mut().iter_mut().find(|p| p.number() == number).ok_or("poule introuvable")?;
                    let m = pool.matches().get(bout).ok_or("match introuvable")?;
                    let fencers = [m.bout.opponent(0).ok_or("match incomplet")?, m.bout.opponent(1).ok_or("match incomplet")?];
                    match_xml(&m.bout, &values).ok_or("score incomplet ou invalide")?;
                    for (f, v) in fencers.iter().zip(values.iter()) {
                        pool.set_score_str(players, bout, *f, v.trim());
                    }
                    if pool.matches().get(bout).map(|m| m.bout.has_error()).unwrap_or(false) {
                        return Err("scores incohérents (un seul vainqueur, au plus le score maximum)".into());
                    }
                    drop(round);
                    self.dirty = true;
                    return Ok(());
                }
                Err("poule introuvable".into())
            }
            Some("t") => {
                let (set_id, path) = (parts.get(3).ok_or("jeton invalide")?.to_string(), parts.get(4).ok_or("jeton invalide")?.to_string());
                for i in 0..self.schedule.len() {
                    let Some(stage) = self.schedule.stage(i) else { continue };
                    let Some(sup) = stage.as_any().downcast_ref::<TableSupervisor>() else { continue };
                    if stage.base().net_id != stage_id {
                        continue;
                    }
                    if stage.base().locked {
                        return Err("le tableau est terminé".into());
                    }
                    let set = sup.table_set_by_id(&set_id).ok_or("tableau introuvable")?;
                    let node = sup.table_set(set).node_from_path(&path).ok_or("match introuvable")?;
                    let m = sup.table_set(set).bout(node).ok_or("match introuvable")?;
                    let xml = match_xml(m, &values).ok_or("score incomplet ou invalide")?;
                    let applied = self.with_schedule(|s, ctx| {
                        let Some(sup) = s.stage_mut(i).and_then(|x| x.as_any_mut().downcast_mut::<TableSupervisor>()) else { return false };
                        sup.load_remote_score(ctx, set, &path, &xml)
                    });
                    if !applied {
                        return Err("score refusé".into());
                    }
                    self.dirty = true;
                    return Ok(());
                }
                Err("tableau introuvable".into())
            }
            // Lobby : `bout` = manche, `values` = [tireur, place (« A » : abandon, vide : effacée)].
            Some("l") => {
                let k: usize = parts.get(3).and_then(|s| s.parse().ok()).ok_or("jeton invalide")?;
                let player: PlayerRef = values[0].trim().parse().map_err(|_| "tireur invalide")?;
                let raw = values[1].trim().to_uppercase();
                let finish = if raw.is_empty() {
                    None
                } else if raw.starts_with('A') {
                    Some(crate::rounds::lobby::ABANDON)
                } else {
                    Some(raw.parse::<i32>().ok().filter(|p| *p > 0).ok_or("place invalide")?)
                };
                for i in 0..self.schedule.len() {
                    let Some(stage) = self.schedule.stage_mut(i) else { continue };
                    if stage.base().net_id != stage_id {
                        continue;
                    }
                    if stage.base().locked {
                        return Err("la phase est terminée".into());
                    }
                    let l = stage.as_any_mut().downcast_mut::<crate::rounds::lobby::Lobby>().ok_or("lobby introuvable")?;
                    if !l.lobbies().get(k).is_some_and(|m| m.contains(&player)) {
                        return Err("tireur hors du lobby".into());
                    }
                    l.set_finish(k, bout, player, finish);
                    self.dirty = true;
                    return Ok(());
                }
                Err("lobby introuvable".into())
            }
            _ => Err("jeton invalide".into()),
        }
    }
}

/// Jeton d'un lobby (série) : feuille d'un lobby sur le téléphone de l'arbitre.
pub fn lobby_token(contest: u32, stage: u32, lobby: usize) -> String {
    format!("l~{contest}~{stage}~{lobby}")
}


impl Contest {
    /// Membres d'une équipe (« NOM Prénom »), pour la composition de la rencontre.
    pub fn team_members(&self, team: crate::player::PlayerRef) -> Vec<String> {
        self.players
            .get(team)
            .map(|t| {
                t.members
                    .iter()
                    .filter_map(|m| self.players.get(*m))
                    .map(|p| format!("{} {}", p.name(), p.str_attr("first_name").unwrap_or_default()).trim().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Enregistre le chronomètre arrêté et le journal des cartons sans valider le score.
    pub fn smartpoule_apply_officiating(&mut self, token: &str, bout: usize, state: crate::officiating::Officiating) -> Result<(), String> {
        self.smartpoule_edit_match(token, bout, move |m, timing| apply_officiating(m, timing, state))
    }

    /// Rencontre par équipes : enregistre le journal des relais (équipe du
    /// haut, composition, scores cumulés) sans valider le score.
    pub fn smartpoule_apply_relay(&mut self, token: &str, bout: usize, log: crate::relay::RelayLog) -> Result<(), String> {
        self.smartpoule_edit_match(token, bout, move |m, _| {
            log.validate()?;
            if let Some(old) = &m.relay {
                if old == &log { return Ok(()); }
                if log.revision <= old.revision { return Err("une version plus récente de la rencontre existe : recharger sa feuille".into()); }
            }
            if log.scores().is_some_and(|s| s.iter().any(|x| *x > m.max_score)) {
                return Err(format!("score au-delà du maximum de la rencontre ({})", m.max_score));
            }
            m.relay = Some(log);
            Ok(())
        })
    }

    /// Match désigné par un jeton SmartPoule, modifié par `edit`.
    fn smartpoule_edit_match(&mut self, token: &str, bout: usize, edit: impl FnOnce(&mut Match, crate::officiating::BoutTiming) -> Result<(), String>) -> Result<(), String> {
        let parts: Vec<_> = token.split('~').collect();
        if parts.get(1).and_then(|s| s.parse::<u32>().ok()) != Some(self.net_id) { return Err("compétition invalide".into()); }
        let stage_id = parts.get(2).and_then(|s| s.parse::<u32>().ok()).ok_or("jeton invalide")?;
        for i in 0..self.schedule.len() {
            let stage = self.schedule.stage(i).ok_or("phase introuvable")?;
            let timing = stage.bout_timing();
            if parts.first() == Some(&"p") {
                let Some(a) = stage.as_any().downcast_ref::<PoolAllocatorStage>() else { continue };
                let shared = a.shared_round();
                if a.round().net_id() != stage_id { continue; }
                if self.schedule.stage(i+1).is_none_or(|s| s.base().locked) { return Err("tour verrouillé".into()); }
                let number = parts.get(3).and_then(|s| s.parse::<u32>().ok()).ok_or("poule invalide")?;
                let mut round = shared.lock().unwrap_or_else(|e| e.into_inner());
                let pool = round.pools_mut().iter_mut().find(|p| p.number() == number).ok_or("poule introuvable")?;
                let m = pool.bout_mut(bout).ok_or("match introuvable")?;
                edit(m, timing)?;
                drop(round);
                self.dirty = true;
                return Ok(());
            } else if parts.first() == Some(&"t") {
                if stage.base().net_id != stage_id || !stage.as_any().is::<TableSupervisor>() { continue; }
                if stage.base().locked { return Err("tableau verrouillé".into()); }
                if bout != 0 { return Err("index de match invalide".into()); }
                let set_id = *parts.get(3).ok_or("tableau invalide")?;
                let path = *parts.get(4).ok_or("match invalide")?;
                let sup = self.schedule.stage_mut(i).and_then(|s| s.as_any_mut().downcast_mut::<TableSupervisor>()).ok_or("tableau introuvable")?;
                let set = sup.table_set_by_id(set_id).ok_or("tableau introuvable")?;
                let node = sup.table_set(set).node_from_path(path).ok_or("match introuvable")?;
                let m = sup.table_set_mut(set).bout_mut(node).ok_or("match introuvable")?;
                edit(m, timing)?;
                self.dirty = true;
                return Ok(());
            }
        }
        Err("match introuvable".into())
    }
}

fn apply_officiating(m: &mut Match, timing: crate::officiating::BoutTiming, state: crate::officiating::Officiating) -> Result<(), String> {
    state.validate(timing, m.max_score)?;
    if let Some(old) = &m.officiating {
        if old == &state { return Ok(()); }
        if state.revision <= old.revision { return Err("une version plus récente du match existe : recharger sa feuille".into()); }
        for previous in &old.cards {
            let Some(next) = state.cards.iter().find(|card| card.id == previous.id) else { return Err("un carton ne peut être effacé ; l'annuler dans le journal".into()); };
            let mut expected = previous.clone(); expected.cancelled = next.cancelled;
            if expected != *next { return Err("le détail d'un carton déjà enregistré ne peut être remplacé".into()); }
        }
    }
    m.officiating = Some(state);
    Ok(())
}

#[cfg(test)]
mod pin_tests {
    use super::*;

    #[test]
    fn pin_follows_the_key() {
        let key = new_referee_key();
        assert!(is_referee_key(&key));
        let pin = referee_pin(&key);
        assert_eq!(pin.len(), 4);
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(pin, referee_pin(&key));
        assert_eq!(referee_pin("abc"), referee_pin("abc"));
    }
}
