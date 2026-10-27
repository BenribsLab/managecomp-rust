//! Échanges de BellePoule avec le Marshaller, sans réseau
//! (`Object::Disclose/FeedParcel` des compétitions, arbitres, tireurs, tours
//! de poules et poules, et traitement de `BellePoule2D::Roadmap` /
//! `BellePoule2D::Referee`).
//!
//! Le cœur décrit les colis à diffuser ([`Parcel`]) ; l'application les
//! convertit en messages réseau et ne diffuse que ce qui a changé.

use crate::bout::StartTime;
use crate::contest::players::{load_player, write_player_attributes};
use crate::contest::Contest;
use crate::player::{PlayerClass, PlayerRef};
use crate::rounds::pool::{PoolAllocatorStage, PoolSupervisorStage};
use crate::rounds::table::TableSupervisor;
use crate::stage::Stage;
use crate::xml::XmlWriter;

/// Colis à diffuser : nom du message, identifiant réseau et champs du corps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parcel {
    pub name: &'static str,
    pub net_id: u32,
    pub fields: Vec<(String, String)>,
}

impl Parcel {
    fn new(name: &'static str, net_id: u32) -> Self {
        Parcel { name, net_id, fields: Vec::new() }
    }

    fn set(&mut self, key: &str, value: impl ToString) {
        self.fields.push((key.to_string(), value.to_string()));
    }
}

/// Identifiant réseau stable d'un joueur d'une compétition (le C++ tirait un
/// nombre au hasard à chaque lancement ; ici il ne dépend que de la
/// compétition et du joueur, pour remplacer le même colis d'une diffusion à
/// l'autre).
fn player_net_id(contest: u32, kind: u32, reference: PlayerRef) -> u32 {
    let mut x = (contest as u64) << 32 | (kind as u64) << 24 | reference as u64;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x as u32).max(1)
}

impl Contest {
    /// Colis de la compétition, dans l'ordre où le Marshaller doit les
    /// recevoir : compétition, arbitres, tireurs (après l'appel), puis pour
    /// chaque tour de poules commencé le lot et ses poules.
    pub fn parcels(&self) -> Vec<Parcel> {
        let mut out = Vec::new();

        // `Contest::FeedParcel`
        let mut competition = Parcel::new("BellePoule::Competition", self.net_id);
        competition.set("color", &self.color);
        competition.set("weapon", self.weapon.xml_image);
        competition.set("gender", self.gender_code());
        competition.set("category", self.category.xml_image());
        out.push(competition);

        // `RefereesList::Disclose ("BellePoule::Referee")`
        for r in self.referees.refs() {
            let Some(p) = self.referees.get(*r) else { continue };
            let mut parcel = Parcel::new("BellePoule::Referee", player_net_id(self.net_id, 2, *r));
            let mut w = XmlWriter::new().without_indent();
            w.start(PlayerClass::Referee.xml_tag());
            write_player_attributes(p, &mut w, &self.extra_attributes, true);
            w.end();
            parcel.set("xml", w.finish());
            out.push(parcel);
        }

        // `CheckinSupervisor::OnLocked` : `Disclose ("BellePoule::Fencer")`
        let checkin_locked = self.schedule.stage(0).map(|s| s.base().locked).unwrap_or(false);
        if checkin_locked {
            for p in self.players.iter().filter(|p| matches!(p.class, PlayerClass::Fencer | PlayerClass::Team)) {
                let mut parcel = Parcel::new("BellePoule::Fencer", player_net_id(self.net_id, 1, p.reference));
                let mut w = XmlWriter::new().without_indent();
                w.start(p.class.xml_tag());
                write_player_attributes(p, &mut w, &self.extra_attributes, true);
                w.end();
                parcel.set("xml", w.finish());
                parcel.set("competition", self.net_id);
                out.push(parcel);
            }
        }

        // Tours de poules : `Allocator::FeedParcel` puis `Pool::FeedParcel`.
        let current = self.schedule.current_index();
        for i in 0..self.schedule.len() {
            if i > current {
                break;
            }
            let Some(stage) = self.schedule.stage(i) else { continue };
            let Some(allocator) = stage.as_any().downcast_ref::<PoolAllocatorStage>() else { continue };
            let done = self.schedule.stage(i + 1).map(|s| s.base().locked).unwrap_or(false);
            let name = allocator.name();
            let round = allocator.round();
            if round.pools().is_empty() {
                continue;
            }
            let batch_id = round.net_id();
            let mut batch = Parcel::new("BellePoule::Batch", batch_id);
            batch.set("competition", self.net_id);
            batch.set("stage", batch_id);
            batch.set("name", name);
            batch.set("done", done as u32);
            batch.set("ready_jobs", round.pools().len());
            batch.set("expected_jobs", round.pools().len());
            out.push(batch);

            for pool in round.pools() {
                let mut job = Parcel::new("BellePoule::Job", pool.net_id);
                job.set("kind", "pool");
                job.set("competition", self.net_id);
                job.set("stage", batch_id);
                job.set("batch", batch_id);
                let mut w = XmlWriter::new().without_indent();
                pool.save(&self.players, &mut w);
                job.set("xml", w.finish());
                job.set("display_position", pool.number());
                // Entraînement des jeunes : groupe de la poule (placement par groupe).
                if let Some(g) = pool.group() {
                    job.set("group", g);
                }
                job.set("duration_sec", pool.duration_sec());
                job.set("workload_units", pool.workload());
                out.push(job);
            }
        }

        // Lobbys (série) : un lot par phase, un « match » par lobby, placé comme une poule.
        for i in 0..self.schedule.len() {
            if i > current {
                break;
            }
            let Some(stage) = self.schedule.stage(i) else { continue };
            let Some(lobby) = stage.as_any().downcast_ref::<crate::rounds::lobby::Lobby>() else { continue };
            if lobby.lobbies().is_empty() || stage.base().net_id == 0 {
                continue;
            }
            let batch_id = stage.base().net_id;
            let mut batch = Parcel::new("BellePoule::Batch", batch_id);
            batch.set("competition", self.net_id);
            batch.set("stage", batch_id);
            batch.set("name", if stage.name().is_empty() { crate::i18n::tr("Lobby (series)") } else { stage.name() });
            batch.set("done", stage.base().locked as u32);
            batch.set("ready_jobs", lobby.lobbies().len());
            batch.set("expected_jobs", lobby.lobbies().len());
            out.push(batch);
            for l in 0..lobby.lobbies().len() {
                let mut job = Parcel::new("BellePoule::Job", lobby.lobby_net_id(l));
                job.set("kind", "pool");
                job.set("competition", self.net_id);
                job.set("stage", batch_id);
                job.set("batch", batch_id);
                job.set("xml", lobby.job_xml(l));
                job.set("display_position", l + 1);
                job.set("duration_sec", lobby.duration_sec());
                job.set("workload_units", lobby.settings.races.max(1));
                out.push(job);
            }
        }

        // Tableaux : `Table::Spread` (lot par tableau prêt) puis `Match::FeedParcel`.
        for i in 0..self.schedule.len() {
            if i > current {
                break;
            }
            let Some(stage) = self.schedule.stage(i) else { continue };
            let Some(sup) = stage.as_any().downcast_ref::<TableSupervisor>() else { continue };
            let stage_id = stage.base().net_id;
            for set in sup.table_sets().iter().filter(|s| s.is_active()) {
                for table in set.tables().iter().skip(1) {
                    if !(table.ready_to_fence || table.is_over) {
                        continue;
                    }
                    let ready: Vec<_> = table
                        .match_list
                        .iter()
                        .filter_map(|n| set.bout(*n))
                        .filter(|m| m.opponent(0).is_some() && m.opponent(1).is_some())
                        .collect();
                    let mut batch = Parcel::new("BellePoule::Batch", table.net_id);
                    batch.set("competition", self.net_id);
                    batch.set("stage", stage_id);
                    batch.set("batch", set.id());
                    batch.set("name", format!("{}:T{}", set.first_place(), table.size));
                    batch.set("done", table.is_over as u32);
                    batch.set("ready_jobs", ready.len());
                    batch.set("expected_jobs", ready.len());
                    out.push(batch);
                    for m in ready {
                        let mut job = Parcel::new("BellePoule::Job", m.net_id);
                        job.set("competition", self.net_id);
                        job.set("stage", stage_id);
                        job.set("batch", table.net_id);
                        let mut w = XmlWriter::new().without_indent();
                        m.save(&self.players, &mut w);
                        job.set("xml", w.finish());
                        job.set("workload_units", m.max_score);
                        job.set("duration_sec", m.duration_sec);
                        job.set("duration_span", m.duration_span);
                        job.set("display_position", m.number);
                        out.push(job);
                    }
                }
            }
        }
        out
    }

    /// `TableSet::OnMessage` (`BellePoule2D::Roadmap`) : piste, heure et
    /// arbitres d'un match de tableau (`batch` = tableau, `source` = match).
    #[allow(clippy::too_many_arguments)]
    pub fn apply_table_roadmap(
        &mut self,
        batch: u32,
        source: u32,
        fitness: u32,
        piste: u32,
        date: Option<&str>,
        time: Option<&str>,
        duration_span: u32,
        referees: &[u32],
    ) -> bool {
        let known: Vec<u32> = referees.iter().copied().filter(|r| self.referees.get(*r).is_some()).collect();
        let start = match (date, time) {
            (Some(d), Some(t)) => StartTime::parse(d, t),
            _ => None,
        };
        for i in 0..self.schedule.len() {
            let Some(sup) = self.schedule.stage_mut(i).and_then(|s| s.as_any_mut().downcast_mut::<TableSupervisor>()) else { continue };
            let Some((set, node)) = sup.find_match_by_net_id(batch, source) else { continue };
            let roadmap = crate::rounds::table::table_set::Roadmap { piste, start_time: start, duration_span: duration_span.max(1), referees: known };
            sup.apply_roadmap(set, node, if fitness > 0 { Some(&roadmap) } else { None });
            return true;
        }
        false
    }

    /// `Allocator::OnPoolRoadmap` : piste, heure et arbitres d'une poule
    /// (`source` = identifiant réseau de la poule). `fitness` nulle : la
    /// poule n'est plus planifiée. Renvoie vrai si une poule a été trouvée.
    pub fn apply_roadmap(&mut self, source: u32, fitness: u32, piste: u32, date: Option<&str>, time: Option<&str>, referees: &[u32]) -> bool {
        self.apply_pool_roadmap(source, fitness, &[piste], date, time, referees)
    }

    /// Extension de la feuille de route : plusieurs pistes communes.
    pub fn apply_pool_roadmap(&mut self, source: u32, fitness: u32, pistes: &[u32], date: Option<&str>, time: Option<&str>, referees: &[u32]) -> bool {
        let known: Vec<u32> = referees.iter().copied().filter(|r| self.referees.get(*r).is_some()).collect();
        let start = match (date, time) {
            (Some(d), Some(t)) => StartTime::parse(d, t),
            _ => None,
        };
        for i in 0..self.schedule.len() {
            let Some(stage) = self.schedule.stage(i) else { continue };
            let round = if let Some(a) = stage.as_any().downcast_ref::<PoolAllocatorStage>() {
                a.shared_round()
            } else if let Some(s) = stage.as_any().downcast_ref::<PoolSupervisorStage>() {
                s.shared_round()
            } else {
                continue;
            };
            let mut round = round.lock().unwrap_or_else(|e| e.into_inner());
            let players = &mut self.players;
            if let Some(pool) = round.pools_mut().iter_mut().find(|p| p.net_id == source) {
                pool.remove_all_referees();
                if fitness > 0 {
                    for r in known.iter().rev() {
                        pool.add_referee(*r);
                    }
                    pool.set_roadmap_pistes(players, pistes, start);
                } else {
                    pool.set_roadmap(players, 0, None);
                }
                return true;
            }
        }
        // Lobby (série) : placé comme une poule.
        for i in 0..self.schedule.len() {
            let Some(lobby) = self.schedule.stage_mut(i).and_then(|s| s.as_any_mut().downcast_mut::<crate::rounds::lobby::Lobby>()) else { continue };
            if lobby.apply_roadmap(source, fitness, pistes, start, &known) {
                self.dirty = true;
                return true;
            }
        }
        false
    }

    /// `Contest::ManageReferee` (`BellePoule2D::Referee`) : un arbitre de
    /// l'arme de la compétition est ajouté ou mis à jour.
    /// `decrypt` déchiffre l'attribut `cyphered_password` (clé de l'anneau).
    pub fn manage_referee_xml(&mut self, xml: &str, decrypt: &dyn Fn(&str) -> Option<String>) -> bool {
        let Ok(doc) = roxmltree::Document::parse(xml) else { return false };
        let root = doc.root_element();
        if root.tag_name().name() != PlayerClass::Referee.xml_tag() {
            return false;
        }
        let mut referee = load_player(PlayerClass::Referee, root, &self.extra_attributes);
        // `Referee::GiveAnId` : mot de passe (clé du smartphone) déchiffré.
        if let Some(cypher) = referee.str_attr("cyphered_password").map(str::to_string) {
            if let Some(password) = decrypt(&cypher).filter(|p| p.len() == 32) {
                referee.set_str(&crate::player::AttrId::global("password"), &password);
            }
        }
        let weapons = referee.str_attr("weapon").unwrap_or_default().to_ascii_uppercase();
        if !weapons.contains(self.weapon.xml_image) {
            return false;
        }
        let reference = referee.reference;
        match self.referees.get_mut(reference) {
            Some(old) => old.update_from(&referee),
            None => {
                self.referees.add(referee);
            }
        }
        self.dirty = true;
        true
    }
}

impl Contest {
    /// Mots de passe des arbitres (clés des smartphones), pour authentifier
    /// leurs messages (`GetSecretKey`).
    pub fn referee_passwords(&self) -> Vec<(u32, String)> {
        self.referees
            .refs()
            .iter()
            .filter_map(|r| self.referees.get(*r).and_then(|p| p.str_attr("password").map(|k| (*r, k.to_string()))))
            .collect()
    }

    /// Mot de passe d'un arbitre (chiffrement des réponses au smartphone).
    pub fn referee_password(&self, reference: u32) -> Option<String> {
        self.referees.get(reference).and_then(|p| p.str_attr("password").map(str::to_string))
    }

    /// `SmartPoule::ScoreSheetCall` : feuille (XML) d'une poule (`stage` =
    /// tour de poules, `batch` = numéro de poule) ou d'un match de tableau
    /// (`stage` = phase de tableaux, `batch` = suite, `bout` = chemin du match).
    pub fn smartpoule_score_sheet(&mut self, stage: u32, batch: &str, bout: &str) -> Option<String> {
        let mut w = XmlWriter::new().without_indent();
        self.save_header(&mut w);
        let current = self.schedule.current_index();
        let mut found = false;
        for i in 0..=current.min(self.schedule.len().saturating_sub(1)) {
            let Some(st) = self.schedule.stage(i) else { continue };
            if let Some(a) = st.as_any().downcast_ref::<PoolAllocatorStage>() {
                let round = a.round();
                if round.net_id() != stage {
                    continue;
                }
                let number = crate::attribute::parse_leading_int(batch).max(0) as u32;
                let Some(pool) = round.pools().iter().find(|p| p.number() == number) else { return None };
                round.save_header(&mut w);
                pool.save(&self.players, &mut w);
                w.end();
                found = true;
                break;
            }
            if let Some(sup) = st.as_any().downcast_ref::<TableSupervisor>() {
                if st.base().net_id != stage {
                    continue;
                }
                let Some(set) = sup.table_set_by_id(batch) else { return None };
                let Some(node) = sup.table_set(set).node_from_path(bout) else { return None };
                sup.write_score_sheet(&self.players, set, node, &mut w);
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
        w.end();
        Some(w.finish())
    }

    /// `SmartPoule::Score` : score d'un match saisi sur le smartphone.
    pub fn smartpoule_score(&mut self, stage: u32, batch: &str, bout: &str, xml: &str) -> bool {
        let current = self.schedule.current_index();
        for i in 0..=current.min(self.schedule.len().saturating_sub(1)) {
            let Some(st) = self.schedule.stage(i) else { continue };
            if let Some(a) = st.as_any().downcast_ref::<PoolAllocatorStage>() {
                let round = a.shared_round();
                let mut round = round.lock().unwrap_or_else(|e| e.into_inner());
                if round.net_id() != stage {
                    continue;
                }
                // Le superviseur (phase suivante) ne doit pas être verrouillé.
                if self.schedule.stage(i + 1).map(|s| s.base().locked).unwrap_or(true) {
                    return false;
                }
                let number = crate::attribute::parse_leading_int(batch).max(0) as u32;
                let n = crate::attribute::parse_leading_int(bout).max(0) as usize;
                let players = &mut self.players;
                let Some(pool) = round.pools_mut().iter_mut().find(|p| p.number() == number) else { return false };
                return n > 0 && pool.load_match_xml(players, n, xml);
            }
            if st.as_any().downcast_ref::<TableSupervisor>().is_some() {
                if st.base().net_id != stage || st.base().locked {
                    continue;
                }
                return self.with_schedule(|s, ctx| {
                    let Some(sup) = s.stage_mut(i).and_then(|x| x.as_any_mut().downcast_mut::<TableSupervisor>()) else { return false };
                    let Some(set) = sup.table_set_by_id(batch) else { return false };
                    sup.load_remote_score(ctx, set, bout, xml)
                });
            }
        }
        false
    }
}
