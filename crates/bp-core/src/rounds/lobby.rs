//! Lobby (série) — extension propre au portage Rust : course ou jeu (battle
//! royale…) où tous les participants d'un lobby jouent en même temps et
//! arrivent dans un ordre, sans matchs.
//!
//! - Plusieurs lobbys en parallèle : comme les poules, la formule (nombre
//!   de lobbys) se choisit sur l'écran de la phase d'après le nombre de
//!   participants ; répartition équilibrée (serpentin, décalage possible :
//!   club…) ou par force équivalente (les meilleurs ensemble).
//! - Plusieurs manches : à chaque manche, l'ordre d'arrivée du lobby donne
//!   des points (barème réglable : N → 1, ou liste saisie). Ex aequo permis
//!   (même place, mêmes points ; la place suivante est décalée), abandon.
//! - Classement : points, puis victoires, puis meilleure place ; qualifiés
//!   au total (quota de la phase) ou par lobby.
//! - Épreuve par équipes : l'équipe court comme un participant, ou ses
//!   tireurs courent chacun et l'équipe est classée d'après eux (somme des
//!   points, des N meilleurs…), avec un classement individuel en plus.

use std::any::Any;
use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::player::{AttrId, PlayerClass, PlayerRef, PlayerStore};
use crate::rounds::table::common::{self, codes};
use crate::stage::{PreviousStage, RankEntry, Stage, StageBase, StageContext, StageKind};
use crate::xml::XmlWriter;

/// Abandon dans une manche (au lieu d'une place).
pub const ABANDON: i32 = -1;

/// Répartition dans les lobbys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    /// Équilibrée : serpentin d'après le classement d'entrée.
    #[default]
    Balanced,
    /// Par force équivalente : les premiers ensemble dans le lobby 1, etc.
    Level,
}

/// Formules possibles pour `n` participants : nombres de lobbys (au moins
/// deux participants par lobby, un seul lobby toujours possible).
pub fn lobby_choices(n: usize) -> Vec<u32> {
    let mut out = vec![1];
    for k in 2..=(n / 2) {
        out.push(k as u32);
    }
    out
}

/// « 4 lobbys de 8 », « 5 lobbys de 6 ou 7 ».
pub fn choice_label(lobbies: u32, n: usize) -> String {
    let k = lobbies.max(1) as usize;
    let small = n / k;
    let plural = if k > 1 { "s" } else { "" };
    if n % k == 0 {
        format!("{k} lobby{plural} de {small}")
    } else {
        format!("{k} lobby{plural} de {small} ou {}", small + 1)
    }
}

/// Épreuve par équipes : qui court.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamMode {
    /// L'équipe court comme un seul participant.
    #[default]
    Team,
    /// Chaque tireur de l'équipe court ; l'équipe est classée d'après eux.
    Members,
}

/// Classement d'une équipe d'après ses tireurs (mode `Members`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamRule {
    /// Somme des points de tous ses tireurs.
    #[default]
    Sum,
    /// Somme des points de ses `team_best` meilleurs tireurs (1 : le meilleur).
    Best,
}

/// Réglages (préférences de la phase).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbySettings {
    /// Nombre de lobbys en parallèle (choisi sur l'écran de la phase, comme
    /// la formule des poules).
    pub lobbies: u32,
    /// Répartition : équilibrée (serpentin) ou par force équivalente.
    #[serde(default)]
    pub distribution: Distribution,
    /// Nombre de manches.
    pub races: u32,
    /// Points par place (1re, 2e…) ; vide : N points au 1er, 1 au dernier.
    #[serde(default)]
    pub scale: Vec<u32>,
    /// Points d'un abandon.
    #[serde(default)]
    pub abandon_points: u32,
    /// Qualifiés par lobby (sinon : quota de la phase, au total).
    #[serde(default)]
    pub per_lobby: Option<u32>,
    /// Décalage des lobbys (critères, le premier prioritaire).
    #[serde(default)]
    pub swapping: Vec<String>,
    #[serde(default)]
    pub team_mode: TeamMode,
    #[serde(default)]
    pub team_rule: TeamRule,
    /// Mode `Best` : nombre de tireurs comptés.
    #[serde(default = "default_best")]
    pub team_best: u32,
}

fn default_best() -> u32 {
    1
}

impl Default for LobbySettings {
    fn default() -> Self {
        LobbySettings {
            lobbies: 1,
            distribution: Distribution::Balanced,
            races: 1,
            scale: Vec::new(),
            abandon_points: 0,
            per_lobby: None,
            swapping: vec!["club".into()],
            team_mode: TeamMode::Team,
            team_rule: TeamRule::Sum,
            team_best: 1,
        }
    }
}

/// Feuille de route d'un lobby (organisation) : pistes, heure, arbitres.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyRoadmap {
    #[serde(default)]
    pub pistes: Vec<u32>,
    /// `jj.mm.aaaa` et `hh:mm` (format des fichiers).
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub referees: Vec<u32>,
}

impl LobbyRoadmap {
    /// « Piste 3 @ 14:20 » (vide si rien n'est fixé).
    pub fn label(&self) -> String {
        let pistes = self.pistes.iter().map(|p| format!("{p:02}")).collect::<Vec<_>>().join(", ");
        match (pistes.is_empty(), self.time.is_empty()) {
            (false, false) => format!("{} {pistes} @ {}", crate::i18n::tr("Piste"), self.time),
            (false, true) => format!("{} {pistes}", crate::i18n::tr("Piste")),
            (true, false) => format!("@ {}", self.time),
            (true, true) => String::new(),
        }
    }
}

/// Bilan d'un participant (tireur ou équipe) dans son lobby.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    pub points: u32,
    /// Manches gagnées (1re place).
    pub wins: u32,
    /// Meilleure place (0 : aucune).
    pub best: u32,
    /// Manches courues (place ou abandon).
    pub races: u32,
    pub abandons: u32,
}

impl Standing {
    /// Ordre du classement : points, victoires, meilleure place.
    fn cmp_rank(&self, other: &Standing) -> Ordering {
        other
            .points
            .cmp(&self.points)
            .then(other.wins.cmp(&self.wins))
            .then(best_key(self.best).cmp(&best_key(other.best)))
    }
}

fn best_key(best: u32) -> u32 {
    if best == 0 { u32::MAX } else { best }
}

pub struct Lobby {
    base: StageBase,
    previous: Option<PreviousStage>,
    pub settings: LobbySettings,
    /// Participants de chaque lobby (tireurs ou équipes).
    lobbies: Vec<Vec<PlayerRef>>,
    /// Arrivées : [lobby][manche] participant → place (`ABANDON` : abandon).
    results: Vec<Vec<BTreeMap<PlayerRef, i32>>>,
    /// Organisation : piste, heure et arbitres de chaque lobby.
    roadmaps: Vec<LobbyRoadmap>,
    /// Classement figé au verrouillage, et qualifiés.
    result: Option<Vec<PlayerRef>>,
    output_short_list: Option<Vec<PlayerRef>>,
}

impl Lobby {
    pub fn new(id: u32) -> Self {
        let mut base = StageBase::new(StageKind::Lobby, id);
        base.filter = codes(&["stage_start_rank", "name", "first_name", "club"]);
        Lobby { base, previous: None, settings: LobbySettings::default(), lobbies: Vec::new(), results: Vec::new(), roadmaps: Vec::new(), result: None, output_short_list: None }
    }

    /// Identifiant réseau d'un lobby (organisation, téléphones).
    pub fn lobby_net_id(&self, lobby: usize) -> u32 {
        let id = self.base.net_id.wrapping_add((lobby as u32 + 1).wrapping_mul(0x9E37_79B9));
        if id == 0 { 1 } else { id }
    }

    /// Feuille de route d'un lobby.
    pub fn roadmap(&self, lobby: usize) -> Option<&LobbyRoadmap> {
        self.roadmaps.get(lobby).filter(|r| **r != LobbyRoadmap::default())
    }

    /// `BellePoule2D::Roadmap` reçu de l'organisation pour le lobby `net_id`
    /// (`fitness` nulle : plus planifié). Vrai si le lobby est de cette phase.
    pub fn apply_roadmap(&mut self, net_id: u32, fitness: u32, pistes: &[u32], start: Option<crate::bout::StartTime>, referees: &[u32]) -> bool {
        let Some(i) = (0..self.lobbies.len()).find(|i| self.lobby_net_id(*i) == net_id) else { return false };
        self.roadmaps.resize_with(self.lobbies.len(), LobbyRoadmap::default);
        self.roadmaps[i] = if fitness > 0 {
            LobbyRoadmap {
                pistes: pistes.iter().copied().filter(|p| *p > 0).collect(),
                date: start.map(|s| s.xml_date()).unwrap_or_default(),
                time: start.map(|s| s.xml_time()).unwrap_or_default(),
                referees: referees.to_vec(),
            }
        } else {
            LobbyRoadmap::default()
        };
        true
    }

    /// Durée d'un lobby (toutes ses manches), d'après le chronomètre de la phase.
    pub fn duration_sec(&self) -> u32 {
        let t = self.base.bout_timing;
        self.settings.races.max(1) * t.period_seconds.max(60) * t.periods.max(1)
    }

    /// Feuille d'un lobby pour l'organisation : `<Lobby ID=".." Piste..>`
    /// avec ses participants et ses arbitres (comme une poule).
    pub fn job_xml(&self, lobby: usize) -> String {
        let mut w = XmlWriter::new().without_indent();
        w.start("Lobby");
        w.attr_fmt("ID", lobby + 1);
        if let Some(r) = self.roadmap(lobby) {
            if let Some(p) = r.pistes.first() {
                w.attr_fmt("Piste", *p);
            }
            if r.pistes.len() > 1 {
                w.attr("Pistes", &r.pistes.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
            }
            if !r.date.is_empty() && !r.time.is_empty() {
                w.attr("Date", &r.date);
                w.attr("Heure", &r.time);
            }
        }
        for p in self.lobbies.get(lobby).into_iter().flatten() {
            w.start("Tireur");
            w.attr_fmt("REF", *p);
            w.end();
        }
        for r in self.roadmap(lobby).map(|r| r.referees.clone()).unwrap_or_default() {
            w.start("Arbitre");
            w.attr_fmt("REF", r);
            w.end();
        }
        w.end();
        w.finish()
    }

    pub fn lobbies(&self) -> &[Vec<PlayerRef>] {
        &self.lobbies
    }

    /// Les tireurs des équipes courent (mode `Members` d'une épreuve par équipes).
    pub fn members_race(&self, players: &PlayerStore) -> bool {
        self.settings.team_mode == TeamMode::Members && self.base.attendees.iter().any(|r| players.get(*r).is_some_and(|p| p.class == PlayerClass::Team))
    }

    /// Participants qui courent, dans l'ordre d'entrée.
    pub fn competitors(&self, players: &PlayerStore) -> Vec<PlayerRef> {
        if !self.members_race(players) {
            return self.base.attendees.clone();
        }
        let mut out = Vec::new();
        for t in &self.base.attendees {
            let Some(team) = players.get(*t) else { continue };
            for m in &team.members {
                if players.get(*m).is_some_and(|p| p.attending()) && !out.contains(m) {
                    out.push(*m);
                }
            }
        }
        out
    }

    /// Équipe d'un tireur (mode `Members`).
    pub fn team_of(&self, players: &PlayerStore, member: PlayerRef) -> Option<PlayerRef> {
        self.base.attendees.iter().copied().find(|t| players.get(*t).is_some_and(|p| p.members.contains(&member)))
    }

    /// Nouveaux réglages : les lobbys sont recomposés si leur nombre change
    /// (tant qu'aucune arrivée n'est saisie).
    pub fn set_settings(&mut self, ctx: &mut StageContext, settings: LobbySettings) {
        if self.base.locked {
            return;
        }
        let mut settings = settings;
        settings.lobbies = settings.lobbies.clamp(1, 64);
        settings.races = settings.races.clamp(1, 50);
        settings.team_best = settings.team_best.max(1);
        let recompose = settings.lobbies != self.settings.lobbies
            || settings.distribution != self.settings.distribution
            || settings.swapping != self.settings.swapping
            || settings.team_mode != self.settings.team_mode;
        self.settings = settings;
        if recompose && !self.has_results() && !self.base.attendees.is_empty() {
            self.compose(ctx.players);
        }
        self.fit_results();
    }

    /// Formule (nombre de lobbys) et répartition, choisies sur l'écran de la
    /// phase tant qu'aucune arrivée n'est saisie.
    pub fn set_composition(&mut self, ctx: &mut StageContext, lobbies: u32, distribution: Distribution) {
        if self.base.locked || self.has_results() {
            return;
        }
        let mut settings = self.settings.clone();
        settings.lobbies = lobbies.max(1);
        settings.distribution = distribution;
        self.settings = settings;
        if !self.base.attendees.is_empty() {
            self.compose(ctx.players);
        }
    }

    pub fn has_results(&self) -> bool {
        self.results.iter().flatten().any(|r| !r.is_empty())
    }

    /// Une manche par réglage, un lobby par lobby.
    fn fit_results(&mut self) {
        self.results.resize_with(self.lobbies.len(), Vec::new);
        for lobby in self.results.iter_mut() {
            lobby.resize_with(self.settings.races.max(1) as usize, BTreeMap::new);
        }
    }

    /// Compose les lobbys : serpentin d'après le classement d'entrée, puis
    /// décalage (deux participants du même club, par exemple, sont séparés
    /// si un échange dans la même rangée le permet).
    pub fn compose(&mut self, players: &PlayerStore) {
        let competitors = self.competitors(players);
        let n = (self.settings.lobbies.max(1) as usize).min(competitors.len().max(1));
        let mut lobbies: Vec<Vec<PlayerRef>> = vec![Vec::new(); n];
        if self.settings.distribution == Distribution::Level {
            // Par force équivalente : les premiers dans le lobby 1, les suivants dans le 2…
            let total = competitors.len();
            let mut at = 0;
            for (l, lobby) in lobbies.iter_mut().enumerate() {
                let size = total / n + usize::from(l < total % n);
                lobby.extend_from_slice(&competitors[at..at + size]);
                at += size;
            }
            self.lobbies = lobbies;
            self.results.clear();
            self.roadmaps.clear();
            self.fit_results();
            return;
        }
        for (i, r) in competitors.iter().enumerate() {
            let row = i / n;
            let col = if row % 2 == 0 { i % n } else { n - 1 - i % n };
            lobbies[col].push(*r);
        }
        // Décalage : le premier critère qui sépare.
        let value = |r: PlayerRef, code: &str| players.get(r).and_then(|p| p.str_attr(code)).map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty());
        for code in self.settings.swapping.clone() {
            let rows = lobbies.iter().map(Vec::len).max().unwrap_or(0);
            for row in 0..rows {
                for l in 0..n {
                    let Some(r) = lobbies[l].get(row).copied() else { continue };
                    let Some(v) = value(r, &code) else { continue };
                    let clash = lobbies[l].iter().enumerate().any(|(i, x)| i != row && value(*x, &code).as_ref() == Some(&v));
                    if !clash {
                        continue;
                    }
                    // Échange avec un participant de la même rangée d'un autre lobby.
                    for o in 0..n {
                        if o == l {
                            continue;
                        }
                        let Some(other) = lobbies[o].get(row).copied() else { continue };
                        let ov = value(other, &code);
                        let fits_there = !lobbies[o].iter().enumerate().any(|(i, x)| i != row && value(*x, &code).as_ref() == Some(&v));
                        let fits_here = ov.as_ref().is_none_or(|ov| !lobbies[l].iter().enumerate().any(|(i, x)| i != row && value(*x, &code).as_ref() == Some(ov)));
                        if fits_there && fits_here {
                            lobbies[l][row] = other;
                            lobbies[o][row] = r;
                            break;
                        }
                    }
                }
            }
        }
        self.lobbies = lobbies;
        self.results.clear();
        self.roadmaps.clear();
        self.fit_results();
    }

    /// Change le lobby d'un participant (avant toute arrivée).
    pub fn move_competitor(&mut self, r: PlayerRef, to: usize) {
        if self.base.locked || self.has_results() || to >= self.lobbies.len() {
            return;
        }
        for l in self.lobbies.iter_mut() {
            l.retain(|x| *x != r);
        }
        self.lobbies[to].push(r);
    }

    /// Arrivée d'un participant dans une manche : place (1…), `ABANDON`, ou
    /// `None` pour effacer.
    pub fn set_finish(&mut self, lobby: usize, race: usize, r: PlayerRef, finish: Option<i32>) {
        if self.base.locked || !self.lobbies.get(lobby).is_some_and(|l| l.contains(&r)) {
            return;
        }
        self.fit_results();
        let Some(map) = self.results.get_mut(lobby).and_then(|l| l.get_mut(race)) else { return };
        match finish.filter(|f| *f > 0 || *f == ABANDON) {
            Some(f) => {
                map.insert(r, f);
            }
            None => {
                map.remove(&r);
            }
        }
    }

    /// Ordre d'arrivée complet d'une manche (le premier en tête) : places
    /// 1, 2, 3… ; `ties[i]` vrai si le i-ème est ex aequo avec le précédent.
    pub fn set_order(&mut self, lobby: usize, race: usize, order: &[PlayerRef], ties: &[bool], abandons: &[PlayerRef]) {
        if self.base.locked {
            return;
        }
        self.fit_results();
        let Some(members) = self.lobbies.get(lobby).cloned() else { return };
        let Some(map) = self.results.get_mut(lobby).and_then(|l| l.get_mut(race)) else { return };
        map.clear();
        let mut place = 0;
        for (i, r) in order.iter().enumerate() {
            if !members.contains(r) {
                continue;
            }
            if !(i > 0 && ties.get(i).copied().unwrap_or(false)) {
                place = i as i32 + 1;
            }
            map.insert(*r, place);
        }
        for r in abandons {
            if members.contains(r) {
                map.insert(*r, ABANDON);
            }
        }
    }

    pub fn finish(&self, lobby: usize, race: usize, r: PlayerRef) -> Option<i32> {
        self.results.get(lobby)?.get(race)?.get(&r).copied()
    }

    /// Points d'une place dans un lobby de `size` participants.
    pub fn points_for(&self, finish: i32, size: usize) -> u32 {
        if finish == ABANDON {
            return self.settings.abandon_points;
        }
        let place = finish.max(1) as usize;
        if self.settings.scale.is_empty() {
            (size + 1).saturating_sub(place) as u32
        } else {
            self.settings.scale.get(place - 1).copied().unwrap_or(0)
        }
    }

    /// Bilan d'un participant dans son lobby.
    pub fn standing(&self, r: PlayerRef) -> Standing {
        let mut s = Standing::default();
        let Some(l) = self.lobbies.iter().position(|x| x.contains(&r)) else { return s };
        let size = self.lobbies[l].len();
        for race in self.results.get(l).into_iter().flatten() {
            let Some(f) = race.get(&r).copied() else { continue };
            s.races += 1;
            s.points += self.points_for(f, size);
            if f == ABANDON {
                s.abandons += 1;
            } else {
                let place = f as u32;
                if place == 1 {
                    s.wins += 1;
                }
                if s.best == 0 || place < s.best {
                    s.best = place;
                }
            }
        }
        s
    }

    /// Classement d'un lobby : (participant, place, bilan), ex aequo compris.
    pub fn lobby_ranking(&self, lobby: usize) -> Vec<(PlayerRef, u32, Standing)> {
        let Some(members) = self.lobbies.get(lobby) else { return Vec::new() };
        rank(members.iter().map(|r| (*r, self.standing(*r))).collect(), &self.base.attendees_index(members))
    }

    /// Lobby d'un participant.
    pub fn lobby_of(&self, r: PlayerRef) -> Option<usize> {
        self.lobbies.iter().position(|l| l.contains(&r))
    }

    /// Manches terminées d'un lobby (tous ses participants ont une arrivée).
    pub fn races_done(&self, lobby: usize) -> usize {
        let Some(members) = self.lobbies.get(lobby) else { return 0 };
        self.results.get(lobby).map(|races| races.iter().filter(|m| !members.is_empty() && members.iter().all(|r| m.contains_key(r))).count()).unwrap_or(0)
    }

    pub fn lobby_over(&self, lobby: usize) -> bool {
        self.races_done(lobby) >= self.settings.races.max(1) as usize
    }

    /// Classement individuel (tous les tireurs qui courent).
    pub fn individual_ranking(&self, players: &PlayerStore) -> Vec<(PlayerRef, u32, Standing)> {
        let competitors = self.competitors(players);
        let entry: BTreeMap<PlayerRef, usize> = competitors.iter().enumerate().map(|(i, r)| (*r, i)).collect();
        let per_lobby = self.settings.per_lobby.filter(|_| !self.members_race(players));
        let mut list: Vec<(PlayerRef, Standing, u32)> = competitors
            .iter()
            .map(|r| {
                let place_in_lobby = self.lobby_of(*r).and_then(|l| self.lobby_ranking(l).into_iter().find(|(x, _, _)| x == r).map(|(_, p, _)| p)).unwrap_or(u32::MAX);
                (*r, self.standing(*r), place_in_lobby)
            })
            .collect();
        // Par lobby : les premiers de chaque lobby, puis les deuxièmes…
        let by_lobby = per_lobby.is_some();
        list.sort_by(|a, b| {
            let lobby_order = if by_lobby { a.2.cmp(&b.2) } else { Ordering::Equal };
            lobby_order.then(a.1.cmp_rank(&b.1)).then(entry.get(&a.0).cmp(&entry.get(&b.0)))
        });
        let mut out = Vec::new();
        let mut last: Option<(u32, Standing)> = None;
        for (i, (r, s, p)) in list.iter().enumerate() {
            let key = (if by_lobby { *p } else { 0 }, *s);
            let place = match last {
                Some((lp, ls)) if lp == key.0 && ls.cmp_rank(s) == Ordering::Equal => out.last().map(|(_, x, _): &(PlayerRef, u32, Standing)| *x).unwrap_or(1),
                _ => i as u32 + 1,
            };
            last = Some(key);
            out.push((*r, place, *s));
        }
        out
    }

    /// Points d'une équipe d'après ses tireurs (mode `Members`).
    pub fn team_standing(&self, players: &PlayerStore, team: PlayerRef) -> Standing {
        let Some(t) = players.get(team) else { return Standing::default() };
        let mut members: Vec<Standing> = t.members.iter().filter(|m| self.lobby_of(**m).is_some()).map(|m| self.standing(*m)).collect();
        members.sort_by(|a, b| a.cmp_rank(b));
        if self.settings.team_rule == TeamRule::Best {
            members.truncate(self.settings.team_best.max(1) as usize);
        }
        let mut s = Standing::default();
        for m in members {
            s.points += m.points;
            s.wins += m.wins;
            s.races += m.races;
            s.abandons += m.abandons;
            if m.best != 0 && (s.best == 0 || m.best < s.best) {
                s.best = m.best;
            }
        }
        s
    }

    /// Classement de la phase : (participant de la phase, place).
    pub fn compute_classification(&self, players: &PlayerStore) -> Vec<(PlayerRef, u32)> {
        if self.members_race(players) {
            let teams: Vec<(PlayerRef, Standing)> = self.base.attendees.iter().map(|t| (*t, self.team_standing(players, *t))).collect();
            return rank(teams, &self.base.attendees_index(&self.base.attendees)).into_iter().map(|(r, p, _)| (r, p)).collect();
        }
        self.individual_ranking(players).into_iter().map(|(r, p, _)| (r, p)).collect()
    }

    fn rank_of(list: &[(PlayerRef, u32)]) -> impl Fn(PlayerRef) -> u32 + '_ {
        move |r| list.iter().find(|(p, _)| *p == r).map(|(_, rank)| *rank).unwrap_or(0)
    }

    /// Qualifiés par lobby : les `per_lobby` premiers de chaque lobby (ex
    /// aequo compris), dans l'ordre du classement.
    fn per_lobby_output(&self, players: &PlayerStore, list: &[(PlayerRef, u32)]) -> Option<Vec<PlayerRef>> {
        let n = self.settings.per_lobby.filter(|_| !self.members_race(players))?;
        let mut qualified = Vec::new();
        for l in 0..self.lobbies.len() {
            for (r, place, _) in self.lobby_ranking(l) {
                if place <= n {
                    qualified.push(r);
                }
            }
        }
        Some(list.iter().map(|(r, _)| *r).filter(|r| qualified.contains(r)).collect())
    }
}

impl StageBase {
    /// Position d'entrée de chacun (départage final, ordre d'affichage).
    fn attendees_index(&self, members: &[PlayerRef]) -> BTreeMap<PlayerRef, usize> {
        members.iter().map(|r| (*r, self.attendees.iter().position(|x| x == r).unwrap_or(usize::MAX))).collect()
    }
}

/// Classe des bilans : ex aequo à égalité de points, victoires et meilleure
/// place ; à égalité, ordre d'entrée pour l'affichage.
fn rank(mut list: Vec<(PlayerRef, Standing)>, entry: &BTreeMap<PlayerRef, usize>) -> Vec<(PlayerRef, u32, Standing)> {
    list.sort_by(|a, b| a.1.cmp_rank(&b.1).then(entry.get(&a.0).cmp(&entry.get(&b.0))));
    let mut out: Vec<(PlayerRef, u32, Standing)> = Vec::new();
    for (i, (r, s)) in list.iter().enumerate() {
        let place = match out.last() {
            Some((_, p, last)) if last.cmp_rank(s) == Ordering::Equal => *p,
            _ => i as u32 + 1,
        };
        out.push((*r, place, *s));
    }
    out
}

impl Stage for Lobby {
    fn base(&self) -> &StageBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut StageBase {
        &mut self.base
    }

    fn set_previous(&mut self, previous: Option<PreviousStage>) {
        self.previous = previous;
    }

    fn set_attendees(&mut self, ctx: &mut StageContext, attendees: Vec<PlayerRef>) {
        common::retrieve_attendees(&mut self.base, ctx, attendees);
    }

    /// Lobbys composés à l'entrée dans la phase (s'ils ne le sont pas).
    fn garnish(&mut self, ctx: &mut StageContext) {
        if self.base.net_id == 0 {
            self.base.net_id = common::new_net_id();
        }
        let known: Vec<PlayerRef> = self.lobbies.iter().flatten().copied().collect();
        let competitors = self.competitors(ctx.players);
        if self.lobbies.is_empty() || competitors.iter().any(|r| !known.contains(r)) {
            self.compose(ctx.players);
        }
        self.fit_results();
    }

    fn reset(&mut self, ctx: &mut StageContext) {
        common::reset_statuses(&self.base, ctx);
        self.lobbies.clear();
        self.results.clear();
        self.roadmaps.clear();
        self.result = None;
        self.output_short_list = None;
    }

    fn load(&mut self, ctx: &mut StageContext, node: roxmltree::Node) -> Result<(), crate::Error> {
        common::load_configuration(&mut self.base, node);
        if let Some(s) = node.attribute("Reglages").and_then(|v| serde_json::from_str::<LobbySettings>(v).ok()) {
            self.settings = s;
        }
        if let Some(l) = node.attribute("Lobbys").and_then(|v| serde_json::from_str::<Vec<Vec<PlayerRef>>>(v).ok()) {
            self.lobbies = l;
        }
        if let Some(r) = node.attribute("FeuillesDeRoute").and_then(|v| serde_json::from_str::<Vec<LobbyRoadmap>>(v).ok()) {
            self.roadmaps = r;
        }
        if let Some(r) = node.attribute("Arrivees").and_then(|v| serde_json::from_str::<Vec<Vec<Vec<(PlayerRef, i32)>>>>(v).ok()) {
            self.results = r.into_iter().map(|lobby| lobby.into_iter().map(|race| race.into_iter().collect()).collect()).collect();
        }
        self.fit_results();
        let attendees = self.base.attendees.clone();
        common::retrieve_attendees(&mut self.base, ctx, attendees);
        Ok(())
    }

    fn save(&self, _ctx: &StageContext, w: &mut XmlWriter) {
        w.start(StageKind::Lobby.xml_name());
        self.save_configuration(w);
        if !self.lobbies.is_empty() {
            w.attr("Lobbys", &serde_json::to_string(&self.lobbies).unwrap_or_default());
        }
        if self.roadmaps.iter().any(|r| *r != LobbyRoadmap::default()) {
            w.attr("FeuillesDeRoute", &serde_json::to_string(&self.roadmaps).unwrap_or_default());
        }
        if self.has_results() {
            let results: Vec<Vec<Vec<(PlayerRef, i32)>>> = self.results.iter().map(|l| l.iter().map(|r| r.iter().map(|(p, f)| (*p, *f)).collect()).collect()).collect();
            w.attr("Arrivees", &serde_json::to_string(&results).unwrap_or_default());
        }
        w.end();
    }

    fn save_configuration(&self, w: &mut XmlWriter) {
        common::save_configuration(&self.base, w, false);
        w.attr("Reglages", &serde_json::to_string(&self.settings).unwrap_or_default());
    }

    fn is_over(&self, _ctx: &StageContext) -> bool {
        !self.lobbies.is_empty() && (0..self.lobbies.len()).all(|l| self.lobby_over(l))
    }

    fn classification(&self, ctx: &StageContext) -> Vec<RankEntry> {
        let list = self.compute_classification(ctx.players);
        let result: Vec<PlayerRef> = match &self.result {
            Some(r) => r.clone(),
            None => list.iter().map(|(p, _)| *p).collect(),
        };
        let rank_of = Self::rank_of(&list);
        common::rank_entries(ctx.players, self.base.owner(), &result, &rank_of)
    }

    fn output(&self, ctx: &StageContext) -> Vec<PlayerRef> {
        if let Some(o) = &self.output_short_list {
            return o.clone();
        }
        let list = self.compute_classification(ctx.players);
        if let Some(o) = self.per_lobby_output(ctx.players, &list) {
            return o;
        }
        let result: Vec<PlayerRef> = list.iter().map(|(p, _)| *p).collect();
        let output = common::compute_output(&self.base, ctx.players, &result, &Self::rank_of(&list), true).output;
        output
    }

    fn lock(&mut self, ctx: &mut StageContext) {
        self.base.locked = true;
        let previous_output = self.output_short_list.take().unwrap_or_default();
        common::restore_not_qualified(self.base.owner(), ctx, &previous_output);
        let list = self.compute_classification(ctx.players);
        common::write_ranks(self.base.owner(), ctx, &list);
        let result: Vec<PlayerRef> = list.iter().map(|(p, _)| *p).collect();
        let output = match self.per_lobby_output(ctx.players, &list) {
            Some(o) => {
                // Non qualifiés de leur lobby.
                let owner = self.base.owner();
                for r in result.iter().filter(|r| !o.contains(r)) {
                    if let Some(p) = ctx.players.get_mut(*r) {
                        p.set_str(&AttrId::local("status", owner), "N");
                        p.set_str(&AttrId::global("global_status"), "N");
                    }
                }
                self.base.quota_exceedance = 0;
                o
            }
            None => {
                let c = common::compute_output(&self.base, ctx.players, &result, &Self::rank_of(&list), true);
                common::apply_output(&mut self.base, ctx, &c);
                c.output
            }
        };
        self.result = Some(result);
        self.output_short_list = Some(output);
    }

    fn unlock(&mut self, _ctx: &mut StageContext) {
        self.result = None;
        self.output_short_list = None;
        self.base.quota_exceedance = 0;
        self.base.locked = false;
    }

    /// Export HTML (résultats complets) : un tableau par lobby (places de
    /// chaque manche, points, victoires, place), puis le classement de la
    /// phase. Mêmes styles que les poules.
    fn dump_to_html(&self, ctx: &StageContext, html: &mut String) {
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let players = &*ctx.players;
        let name_of = |r: PlayerRef| {
            players
                .get(r)
                .map(|p| (format!("{} {}", p.name(), p.first_name()).trim().to_string(), p.str_attr("club").unwrap_or_default().to_string()))
                .unwrap_or_default()
        };
        let races = self.settings.races.max(1) as usize;
        html.push_str("        <div>
          <table>
");
        for k in 0..self.lobbies.len() {
            let road = self.roadmap(k).map(|r| r.label()).filter(|l| !l.is_empty()).map(|l| format!(" — {l}")).unwrap_or_default();
            html.push_str("          <tr class=\"PoolName\">
");
            html.push_str(&format!("            <td>Lobby {}{}</td>
            <td class=\"GridSeparator\"></td>
", k + 1, esc(&road)));
            for r in 1..=races {
                html.push_str(&format!("            <td class=\"GridSeparator\" style=\"text-align: center\";>M{r}</td>
"));
            }
            for h in [crate::i18n::tr("Points"), crate::i18n::tr("Wins"), "#".to_string()] {
                html.push_str(&format!("            <td class=\"GridSeparator\" style=\"text-align: center\";>{}</td>
", esc(&h)));
            }
            html.push_str("          </tr>
");
            for (r, place, s) in self.lobby_ranking(k) {
                let (name, club) = name_of(r);
                html.push_str("          <tr class=\"EvenRow\">
");
                html.push_str(&format!("            <td>
              <span class=\"name\">{} </span><span class=\"club\">{}</span>            </td>
", esc(&name), esc(&club)));
                html.push_str("            <td class=\"GridSeparator\"></td>
");
                for race in 0..races {
                    let cell = match self.finish(k, race, r) {
                        Some(ABANDON) => "A".to_string(),
                        Some(p) => p.to_string(),
                        None => String::new(),
                    };
                    html.push_str(&format!("            <td class=\"PoolCell\">{cell}</td>
"));
                }
                html.push_str(&format!("            <td class=\"DashBoardCell\">{}</td>
", s.points));
                html.push_str(&format!("            <td class=\"DashBoardCell\">{}</td>
", s.wins));
                html.push_str(&format!("            <td class=\"DashBoardCell\">{}</td>
", if s.races > 0 { place.to_string() } else { String::new() }));
                html.push_str("          </tr>
");
            }
            html.push_str("          <tr class=\"PoolSeparator\">
");
        }
        html.push_str("          </table>

        </div>
");
        // Classement de la phase (lobbys fusionnés).
        if self.has_results() {
            let members = self.members_race(players);
            html.push_str(&format!("        <h3>{}</h3>
        <div>
          <table>
", esc(&crate::i18n::tr("Ranking of the phase"))));
            html.push_str(&format!(
                "          <tr class=\"PoolName\"><td class=\"GridSeparator\">#</td><td class=\"GridSeparator\">{}</td><td class=\"GridSeparator\">{}</td><td class=\"GridSeparator\">{}</td></tr>
",
                esc(&crate::i18n::tr("Name")),
                esc(&crate::i18n::tr("Club")),
                esc(&crate::i18n::tr("Points"))
            ));
            for (r, place) in self.compute_classification(players) {
                let (name, club) = name_of(r);
                let s = if members { self.team_standing(players, r) } else { self.standing(r) };
                html.push_str(&format!(
                    "          <tr class=\"EvenRow\"><td class=\"DashBoardCell\">{place}</td><td><span class=\"name\">{}</span></td><td><span class=\"club\">{}</span></td><td class=\"DashBoardCell\">{}</td></tr>
",
                    esc(&name),
                    esc(&club),
                    s.points
                ));
            }
            html.push_str("          </table>
        </div>
");
        }
    }

    fn remap_players(&mut self, map: &std::collections::HashMap<PlayerRef, PlayerRef>) {
        let tr = |r: &PlayerRef| map.get(r).copied().unwrap_or(*r);
        self.base.attendees = self.base.attendees.iter().map(tr).collect();
        self.lobbies = self.lobbies.iter().map(|l| l.iter().map(tr).collect()).collect();
        self.results = self.results.iter().map(|l| l.iter().map(|race| race.iter().map(|(r, f)| (tr(r), *f)).collect()).collect()).collect();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Fabrique (appelée par `rounds::create_stage`).
pub fn new_stage(_kind: StageKind, id: u32) -> Box<dyn Stage> {
    Box::new(Lobby::new(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contest::players::new_player;
    use crate::stage::ContestSettings;

    fn store(n: u32) -> (PlayerStore, Vec<PlayerRef>) {
        let mut s = PlayerStore::default();
        let mut refs = Vec::new();
        for i in 0..n {
            let mut p = new_player(PlayerClass::Fencer);
            p.set_str(&AttrId::global("name"), &format!("T{i}"));
            p.set_str(&AttrId::global("club"), if i % 2 == 0 { "A" } else { "B" });
            refs.push(s.add(p));
        }
        (s, refs)
    }

    fn lobby(players: &mut PlayerStore, refs: &[PlayerRef], settings: LobbySettings) -> Lobby {
        let mut l = Lobby::new(5);
        l.settings = settings;
        let cs = ContestSettings::default();
        let mut ctx = StageContext { players, settings: &cs };
        l.set_attendees(&mut ctx, refs.to_vec());
        l.garnish(&mut ctx);
        l
    }

    #[test]
    fn serpentine_lobbies_and_points() {
        let (mut players, refs) = store(8);
        let l = lobby(&mut players, &refs, LobbySettings { lobbies: 2, swapping: Vec::new(), ..Default::default() });
        assert_eq!(l.lobbies()[0], vec![refs[0], refs[3], refs[4], refs[7]]);
        assert_eq!(l.lobbies()[1], vec![refs[1], refs[2], refs[5], refs[6]]);
        assert_eq!(l.points_for(1, 4), 4);
        assert_eq!(l.points_for(4, 4), 1);
        assert_eq!(l.points_for(ABANDON, 4), 0);
    }

    #[test]
    fn ties_share_place_and_points_and_ranking_follows() {
        let (mut players, refs) = store(4);
        let mut l = lobby(&mut players, &refs, LobbySettings { races: 2, swapping: Vec::new(), ..Default::default() });
        // Manche 1 : 0, (1 et 2 ex aequo), 3 ; manche 2 : 3, 2, 1, abandon de 0.
        l.set_order(0, 0, &[refs[0], refs[1], refs[2], refs[3]], &[false, false, true, false], &[]);
        assert_eq!(l.finish(0, 0, refs[2]), Some(2));
        assert_eq!(l.finish(0, 0, refs[3]), Some(4));
        l.set_order(0, 1, &[refs[3], refs[2], refs[1]], &[false, false, false], &[refs[0]]);
        let cs = ContestSettings::default();
        let ctx = StageContext { players: &mut players, settings: &cs };
        assert!(l.is_over(&ctx));
        // Points : 0 → 4+0 ; 1 → 3+2 ; 2 → 3+3 ; 3 → 1+4.
        let ranking = l.lobby_ranking(0);
        assert_eq!(ranking[0].0, refs[2]);
        assert_eq!(ranking[0].2.points, 6);
        // 3 et 1 : 5 points ; 3 a une victoire de plus.
        let order: Vec<PlayerRef> = ranking.iter().map(|(r, _, _)| *r).collect();
        assert_eq!(order, vec![refs[2], refs[3], refs[1], refs[0]]);
        let places: Vec<u32> = ranking.iter().map(|(_, p, _)| *p).collect();
        assert_eq!(places, vec![1, 2, 3, 4]);
    }

    #[test]
    fn level_distribution_keeps_the_best_together() {
        let (mut players, refs) = store(7);
        let l = lobby(&mut players, &refs, LobbySettings { lobbies: 2, distribution: Distribution::Level, swapping: Vec::new(), ..Default::default() });
        assert_eq!(l.lobbies()[0], refs[..4].to_vec());
        assert_eq!(l.lobbies()[1], refs[4..].to_vec());
        assert_eq!(lobby_choices(7), vec![1, 2, 3]);
        assert_eq!(choice_label(2, 7), "2 lobbys de 3 ou 4");
    }

    #[test]
    fn per_lobby_qualification() {
        let (mut players, refs) = store(8);
        let mut l = lobby(&mut players, &refs, LobbySettings { lobbies: 2, per_lobby: Some(2), swapping: Vec::new(), ..Default::default() });
        for lobby in 0..2 {
            let order = l.lobbies()[lobby].clone();
            let ties = vec![false; order.len()];
            l.set_order(lobby, 0, &order, &ties, &[]);
        }
        let cs = ContestSettings::default();
        let ctx = StageContext { players: &mut players, settings: &cs };
        let out = l.output(&ctx);
        assert_eq!(out.len(), 4);
        assert!(out.contains(&l.lobbies()[0][0]) && out.contains(&l.lobbies()[1][1]));
    }
}
