//! Réception des messages de BellePoule et placement des matchs
//! (port des parties de `hall.cpp`, `competition.cpp` et `batch.cpp` qui
//! traitent `BellePoule::Competition/Batch/Job/Fencer`, le dépôt d'un match
//! ou d'un arbitre sur une piste, le placement automatique d'un lot et le
//! déplacement d'un match).

use bp_core::{Player, PlayerClass};
use bp_net::Message;

use crate::affinities::Affinities;
use crate::color::Rgb;
use crate::hall::{FloatingJob, Hall};
use crate::model::{Batch, Competition, Fencer, Job, JobRow, JobStatus, UiEvent};
use crate::slot::{JobKey, PisteKey, Slot, SlotKey};
use crate::time::{self, Span};

type LoadedJob = (JobKey, Vec<u32>, Vec<u32>, Option<time::Time>);

/// Choix du dialogue « Placer les matchs » (`job_dialog`).
#[derive(Debug, Clone, PartialEq)]
pub struct AssignOptions {
    /// Tous les matchs en attente (sinon : ceux sélectionnés).
    pub all_jobs: bool,
    /// Matchs sélectionnés dans la liste du lot.
    pub selected_jobs: Vec<JobKey>,
    /// Arbitres : tous, sélectionnés, ou aucun (`no_referee`).
    pub referees: RefereeChoice,
    /// Toutes les pistes (sinon : celles sélectionnées dans la salle).
    pub all_pistes: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RefereeChoice {
    All,
    Selected(Vec<u32>),
    None,
}

impl Default for AssignOptions {
    fn default() -> Self {
        AssignOptions { all_jobs: true, selected_jobs: Vec::new(), referees: RefereeChoice::All, all_pistes: true }
    }
}

impl Hall {
    // --------------------------------------------------------------------------------
    // Messages (`Marshaller::OnMessage`)

    /// Aiguillage d'un message reçu. Renvoie vrai s'il a été traité.
    pub fn on_message(&mut self, message: &Message) -> bool {
        if message.fitness() > 0 {
            if message.is("BellePoule::Competition") {
                self.manage_competition(message);
            } else if message.is("BellePoule::Batch") {
                self.manage_batch(message);
            } else if message.is("BellePoule::Job") {
                self.manage_job(message);
            } else if message.is("BellePoule::Referee") {
                self.manage_referee(message);
            } else if message.is("BellePoule::Fencer") {
                self.manage_fencer(message);
            } else if message.is("SmartPoule::RefereeHandshake") {
                self.manage_handshake(message);
            } else if message.is("SmartPoule::PartnerAppCredentials") {
                self.events.push(UiEvent::PartnerCredentials);
                if let Some(passphrase) = message.get_str("user_app_key") {
                    self.outbox.push(crate::model::NetCommand::ChangePassphrase(passphrase));
                    self.outbox.push(crate::model::NetCommand::AnnounceAvailability);
                }
            } else {
                return false;
            }
            true
        } else if message.is("BellePoule::Competition") {
            self.delete_competition(message);
            true
        } else if message.is("BellePoule::Batch") {
            self.delete_batch(message);
            true
        } else if message.is("BellePoule::Job") {
            self.delete_job(message);
            true
        } else if message.is("BellePoule::Fencer") {
            self.delete_fencer(message);
            true
        } else {
            false
        }
    }

    /// `Hall::ManageCompetition` + `Competition::SetProperties`.
    pub fn manage_competition(&mut self, message: &Message) {
        let id = message.net_id();
        let label = |code: &str| -> String {
            let xml = message.get_str(code).unwrap_or_default();
            bp_core::attribute::registry()
                .by_code(code)
                .map(|d| d.user_image(&xml, bp_core::attribute::Look::LongText))
                .unwrap_or(xml)
        };
        let gender = label("gender");
        let weapon_label = label("weapon");
        let category = label("category");
        let weapon = message.get_str("weapon").unwrap_or_default();
        let color = message.get_str("color").and_then(|c| Rgb::parse(&c)).unwrap_or_default();
        let created = self.competition(id).is_none();
        if created {
            self.competitions.insert(0, Competition::new(id));
            self.timeline.competitions.push(id);
        }
        if let Some(c) = self.competition_mut(id) {
            c.frozen = false;
            c.gender = gender;
            c.weapon_label = weapon_label;
            c.category = category;
            c.weapon = weapon;
            c.color = color;
        }
        if created {
            self.events.push(UiEvent::CompetitionAdded(id));
        }
        self.referees_spread();
    }

    /// `Hall::DeleteCompetition` : retirée si elle n'a plus de lot, sinon gelée.
    pub fn delete_competition(&mut self, message: &Message) {
        let id = message.net_id();
        let Some(c) = self.competition_mut(id) else { return };
        if c.batches.is_empty() {
            self.competitions.retain(|c| c.id != id);
            self.timeline.competitions.retain(|c| *c != id);
            self.events.push(UiEvent::CompetitionRemoved(id));
        } else {
            c.frozen = true;
        }
    }

    /// `Hall::ManageBatch` + `Competition::ManageBatch`.
    pub fn manage_batch(&mut self, message: &Message) {
        let c = message.get_uint("competition");
        let id = message.net_id();
        let Some(comp) = self.competition_mut(c) else { return };
        let created = comp.batch(id).is_none();
        if created {
            comp.batches.push(Batch::new(message, c));
        }
        if message.get_uint("done") != 0 {
            self.mask_batch(c, id);
        } else {
            let (expected, ready) = (message.get_uint("expected_jobs"), message.get_uint("ready_jobs"));
            if let Some(comp) = self.competition_mut(c) {
                if let Some(b) = comp.batch_mut(id) {
                    b.panel.visible = true;
                    b.panel.expected = expected;
                    b.panel.ready = ready;
                }
                if comp.current_batch.is_none() {
                    comp.current_batch = Some(id);
                }
                comp.frozen = false;
            }
        }
        if created {
            self.events.push(UiEvent::BatchAdded { competition: c, batch: id });
        }
        self.competition_on_new_batch_status(c);
    }

    /// `Competition::MaskBatch`.
    fn mask_batch(&mut self, c: u32, batch: u32) {
        let Some(comp) = self.competition_mut(c) else { return };
        if let Some(b) = comp.batch_mut(batch) {
            b.panel.visible = false;
        }
        if comp.current_batch == Some(batch) {
            comp.current_batch = comp.batches.iter().rev().find(|b| b.panel.visible).map(|b| b.id);
            if comp.current_batch.is_none() {
                comp.frozen = true;
            }
        }
    }

    /// `Hall::DeleteBatch` + `Competition::DeleteBatch`.
    pub fn delete_batch(&mut self, message: &Message) {
        let c = message.get_uint("competition");
        let id = message.net_id();
        if self.batch(c, id).is_none() {
            return;
        }
        self.overlaps.retain(|w| !(w.competition == c && w.batch == id));
        self.mask_batch(c, id);
        let jobs: Vec<JobKey> = self.batch(c, id).map(|b| b.rows.iter().map(|r| r.job).collect()).unwrap_or_default();
        self.batch_mute(c, id, true);
        for j in jobs {
            self.forget_job(j);
        }
        if let Some(comp) = self.competition_mut(c) {
            comp.batches.retain(|b| b.id != id);
        }
        self.events.push(UiEvent::BatchRemoved { competition: c, batch: id });
        self.competition_on_new_batch_status(c);
        self.redraw();
    }

    /// Retire un match de son créneau et de l'arène (`Job::~Job`).
    fn forget_job(&mut self, job: JobKey) {
        if let Some(sk) = self.jobs.get(&job).and_then(|j| j.slot) {
            self.slot_remove_job(sk, job);
        }
        self.jobs.remove(&job);
        self.gc_slots();
    }

    /// `Hall::ManageJob` + `Batch::Load`.
    pub fn manage_job(&mut self, message: &Message) {
        let c = message.get_uint("competition");
        let b = message.get_uint("batch");
        let Some(comp) = self.competition(c) else { return };
        let weapon = comp.weapon.clone();
        let color = comp.color;
        if comp.batch(b).is_none() {
            return;
        }
        let real_duration = message.get_uint("duration_sec") as Span * time::SECOND;
        let netid = message.net_id();
        if let Some(existing) = self.job_by_netid(c, b, netid) {
            if let Some(j) = self.jobs.get_mut(&existing) {
                j.group = Some(message.get_uint("group")).filter(|g| *g > 0);
            }
            self.job_set_real_duration(existing, real_duration);
            self.redraw();
            return;
        }

        self.batch_mute(c, b, true);
        let loaded = self.load_job(message, c, b, color);
        if let Some((job, piste_ids, referees, start)) = loaded {
            if let Some(w) = crate::weapon::from_xml(&weapon) {
                if let Some(j) = self.jobs.get_mut(&job) {
                    j.set_weapon(w);
                }
            }
            if let Some(start) = start.filter(|_| !piste_ids.is_empty() || !referees.is_empty()) {
                let mut pistes = Vec::new();
                for piste_id in &piste_ids {
                let piste = match self.piste_by_id(*piste_id) {
                    Some(p) => p,
                    None => {
                        let p = self.add_piste(0.0, 0.0);
                        self.set_piste_id(p, *piste_id);
                        p
                    }
                };
                pistes.push(piste);
                }
                let duration = self.jobs.get(&job).map(|j| j.duration()).unwrap_or(0);
                let reserved = if let Some(piste) = pistes.first().copied() { self.piste_create_slot(piste, start, duration) }
                    else { Some(self.slot_insert(Slot::new(None, start, None, duration))) };
                if let Some(sk) = reserved {
                    self.slot_add_job(sk, job);
                    for r in referees.iter().rev() {
                        if self.referees.referees.contains_key(r) {
                            self.slot_add_referee(sk, *r);
                        }
                    }
                    if pistes.len() > 1 {
                        let known = self.slots.get(&sk).map(|s| s.referees.clone()).unwrap_or_default();
                        if let Err(error) = self.set_pool_resources_optional(job, &pistes, &known, false) {
                            self.slot_cancel(sk);
                            log::warn!("Pool resources: {error}");
                        }
                    }
                    self.job_set_real_duration(job, real_duration);
                    self.redraw();
                }
            }
        }
        self.batch_mute(c, b, false);
        self.refresh_workload(&weapon);
    }

    /// Match d'un lot d'après l'identifiant réseau du message qui l'a créé.
    pub fn job_by_netid(&self, c: u32, b: u32, netid: u32) -> Option<JobKey> {
        let batch = self.batch(c, b)?;
        batch.rows.iter().map(|r| r.job).find(|j| self.jobs.get(j).map(|x| x.netid == netid).unwrap_or(false))
    }

    /// `Batch::Load` : crée le match (poule ou match de tableau) et renvoie la
    /// piste, les arbitres et l'heure déjà connus.
    fn load_job(&mut self, message: &Message, c: u32, b: u32, color: Rgb) -> Option<LoadedJob> {
        let xml = message.get_str("xml")?;
        let doc = roxmltree::Document::parse(&xml).ok()?;
        let root = doc.root_element();
        let heading = match root.tag_name().name() {
            "Poule" => "Pool",
            "Match" => "Match",
            // Lobby (série) : placé comme une poule.
            "Lobby" => "Lobby",
            _ => return None,
        };
        let key = self.next_job;
        self.next_job += 1;
        let mut job = Job::new(key, c, b, message, color);
        job.is_pool = matches!(root.tag_name().name(), "Poule" | "Lobby");
        job.name = match root.attribute("ID") {
            Some(id) => format!("{} {}", bp_core::i18n::tr(heading), id),
            None => bp_core::i18n::tr("Pool"),
        };
        let piste_id = root.attribute("Piste").map(|p| bp_core::attribute::parse_leading_int(p).max(0) as u32).unwrap_or(0);
        let mut piste_ids = Vec::new();
        if piste_id > 0 { piste_ids.push(piste_id); }
        if job.is_pool {
            for id in root.attribute("Pistes").unwrap_or_default().split(',').filter_map(|s| s.trim().parse::<u32>().ok()) {
                if id > 0 && !piste_ids.contains(&id) { piste_ids.push(id); }
            }
        }
        let start = match (root.attribute("Date"), root.attribute("Heure")) {
            (Some(d), Some(t)) => time::parse_fie(d, t),
            _ => None,
        };
        let mut referees = Vec::new();
        for n in root.children().filter(|n| n.is_element()) {
            let reference = n.attribute("REF").map(|r| bp_core::attribute::parse_leading_int(r).max(0) as u32);
            match (n.tag_name().name(), reference) {
                ("Tireur" | "Equipe", Some(r)) => {
                    if let Some(f) = self.competition(c).and_then(|comp| comp.fencer(r)) {
                        job.fencers.push(f.clone());
                    }
                }
                ("Arbitre", Some(r)) => referees.push(r),
                _ => {}
            }
        }
        self.jobs.insert(key, job);
        if let Some(batch) = self.batch_mut(c, b) {
            batch.rows.push(JobRow { job: key, status: JobStatus::Question, warning: None });
            batch.pending.push(key);
        }
        self.sort_batch_rows(c, b);
        self.batch_refresh_control_panel(c, b);
        if let Some(mapping) = root.attribute("ArbitresPistes") {
            let pairs: Vec<(u32, u32)> = mapping.split(',').filter_map(|s| {
                let (p, r) = s.split_once(':')?; Some((p.parse().ok()?, r.parse().ok()?))
            }).collect();
            let ordered: Vec<_> = piste_ids.iter().filter_map(|p| pairs.iter().find(|(key, _)| key == p).map(|(_, r)| *r)).collect();
            if ordered.len() == piste_ids.len() && piste_ids.len() > 1 { referees = ordered; }
        }
        Some((key, piste_ids, referees, start))
    }

    /// Lignes de la liste du lot triées par position d'affichage.
    fn sort_batch_rows(&mut self, c: u32, b: u32) {
        let positions: std::collections::HashMap<JobKey, u32> = self.jobs.iter().map(|(k, j)| (*k, j.position)).collect();
        if let Some(batch) = self.batch_mut(c, b) {
            batch.rows.sort_by_key(|r| positions.get(&r.job).copied().unwrap_or(0));
        }
    }

    /// `Piste::CreateSlot` : créneau libre commençant à `start` (sinon aucun).
    pub fn piste_create_slot(&mut self, piste: PisteKey, start: time::Time, duration: Span) -> Option<SlotKey> {
        let free = self.piste_free_slot(piste, start, duration)?;
        Some(self.slot_insert(free))
    }

    /// `Hall::DeleteJob` + `Batch::RemoveJob`.
    pub fn delete_job(&mut self, message: &Message) {
        let c = message.get_uint("competition");
        let b = message.get_uint("batch");
        let Some(weapon) = self.competition(c).map(|x| x.weapon.clone()) else { return };
        let Some(job) = self.job_by_netid(c, b, message.net_id()) else { return };
        self.batch_mute(c, b, true);
        if let Some(batch) = self.batch_mut(c, b) {
            batch.rows.retain(|r| r.job != job);
            batch.pending.retain(|j| *j != job);
            batch.partly.retain(|j| *j != job);
            batch.scheduled.retain(|j| *j != job);
        }
        self.batch_refresh_control_panel(c, b);
        self.forget_job(job);
        self.events.push(UiEvent::CloseJobBoard);
        self.batch_mute(c, b, false);
        self.refresh_workload(&weapon);
        self.redraw();
    }

    /// `Competition::ManageFencer` : tireur (`/Tireur`) ou équipe (`/Equipe`).
    pub fn manage_fencer(&mut self, message: &Message) {
        let c = message.get_uint("competition");
        let Some(xml) = message.get_str("xml") else { return };
        let Ok(doc) = roxmltree::Document::parse(&xml) else { return };
        let root = doc.root_element();
        let class = match root.tag_name().name() {
            "Tireur" => PlayerClass::Fencer,
            "Equipe" => PlayerClass::Team,
            _ => return,
        };
        let mut player = Player::new(class);
        player.load(root);
        let mut affinities = Affinities::new(&player);
        let mut members = Vec::new();
        if class == PlayerClass::Team {
            for child in root.children().filter(|n| n.is_element()) {
                let mut member = Player::new(PlayerClass::Fencer);
                member.load(child);
                let a = Affinities::new(&member);
                affinities.add_shareholder(a.clone());
                members.push((member, a));
            }
        }
        let netid = message.net_id();
        let Some(comp) = self.competition_mut(c) else { return };
        comp.fencers.retain(|f| f.netid != netid);
        comp.fencers.insert(0, Fencer { player, netid, affinities, members });
    }

    /// `Competition::DeleteFencer`.
    pub fn delete_fencer(&mut self, message: &Message) {
        let c = message.get_uint("competition");
        let netid = message.net_id();
        if let Some(comp) = self.competition_mut(c) {
            comp.fencers.retain(|f| f.netid != netid);
        }
    }

    // --------------------------------------------------------------------------------
    // Glisser-déposer sur une piste

    /// `Hall::DroppingIsAllowed` pour un match.
    pub fn job_drop_allowed(&self, job: JobKey, piste: PisteKey) -> bool {
        let Some(j) = self.jobs.get(&job) else { return false };
        if j.slot.is_some() {
            return self.can_add_pool_piste(job, piste);
        }
        let cursor = self.timeline.cursor_time(false);
        self.piste_free_slot(piste, cursor, j.duration()).is_some()
    }

    /// `Hall::DroppingIsAllowed` pour un arbitre : la piste a un créneau à
    /// l'heure du curseur, de la bonne arme, et l'arbitre est libre.
    pub fn referee_drop_allowed(&self, referee: u32, piste: PisteKey) -> bool {
        let cursor = self.timeline.cursor_time(false);
        let Some(sk) = self.piste_slot_at(piste, cursor) else { return false };
        let Some(slot) = self.slots.get(&sk) else { return false };
        if !slot.additional_pistes.is_empty() { return false; }
        let Some(job) = slot.jobs.first().and_then(|j| self.jobs.get(j)) else { return false };
        let Some(weapon) = self.competition(job.competition).map(|c| c.weapon.clone()) else { return false };
        let Some(r) = self.referees.get(referee) else { return false };
        let has_weapon = r.player.str_attr("weapon").map(|w| w.contains(weapon.as_str())).unwrap_or(false);
        has_weapon && self.referee_is_available(referee, slot, slot.duration)
    }

    /// `Hall::DropObject` (match) : premier créneau libre à partir du curseur.
    pub fn drop_job(&mut self, job: JobKey, piste: PisteKey) {
        // L'ajout d'une piste à une poule existante passe par le dialogue des
        // ressources, qui exige aussi le choix de son arbitre.
        if self.jobs.get(&job).is_some_and(|j| j.slot.is_some()) { return; }
        let Some(duration) = self.jobs.get(&job).map(|j| j.duration()) else { return };
        let from = self.timeline.cursor_time(false);
        let Some(free) = self.piste_free_slots(piste, from, duration).into_iter().next() else { return };
        let sk = self.slot_insert(free);
        self.slot_add_job(sk, job);
        self.redraw();
    }

    /// `Hall::DropObject` (arbitre) → `Piste::AddReferee`.
    pub fn drop_referee(&mut self, referee: u32, piste: PisteKey) {
        let cursor = self.timeline.cursor_time(false);
        if let Some(sk) = self.piste_slot_at(piste, cursor) {
            if self.slots.get(&sk).is_some_and(|s| !s.additional_pistes.is_empty()) { return; }
            self.slot_add_referee(sk, referee);
        }
        self.refresh_referee_workloads(referee);
        self.redraw();
    }

    // --------------------------------------------------------------------------------
    // Placement automatique d'un lot

    /// `Hall::OnBatchAssignmentRequest` (après validation du dialogue).
    pub fn batch_assignment(&mut self, c: u32, b: u32, options: &AssignOptions) {
        let Some(weapon) = self.competition(c).map(|x| x.weapon.clone()) else { return };
        self.events.push(UiEvent::ExposeWeapon(weapon.clone()));
        if self.pistes.is_empty() {
            return;
        }
        let cursor = self.timeline.cursor_time(true);
        self.timeline.set_cursor_time(cursor);

        let jobs = if options.all_jobs {
            self.batch_pending_jobs(c, b)
        } else {
            self.batch_pending_selected(c, b, &options.selected_jobs)
        };

        // Arbitres de l'arme, dans l'ordre de la liste.
        let raw: Vec<u32> = match &options.referees {
            RefereeChoice::All => self
                .referees
                .list_index(&weapon)
                .map(|l| self.referees.lists[l].members.clone())
                .unwrap_or_default(),
            RefereeChoice::Selected(list) => list.clone(),
            RefereeChoice::None => Vec::new(),
        };
        let referee_list: Vec<u32> = raw
            .into_iter()
            .filter(|r| {
                self.referees
                    .get(*r)
                    .and_then(|x| x.player.str_attr("weapon").map(|w| w.contains(weapon.as_str())))
                    .unwrap_or(false)
            })
            .collect();

        self.batch_mute(c, b, true);
        let mut sticky: Vec<SlotKey> = Vec::new();
        for job in &jobs {
            let free_slots: Vec<Slot> = match self.jobs.get(job).and_then(|j| j.slot) {
                Some(sk) => {
                    sticky.push(sk);
                    self.slots.get(&sk).cloned().into_iter().collect()
                }
                None => {
                    let duration = self.jobs.get(job).map(|j| j.duration()).unwrap_or(0);
                    self.free_piste_slots(duration, options.all_pistes)
                }
            };
            let existing = self.jobs.get(job).and_then(|j| j.slot);
            if options.referees == RefereeChoice::None {
                if existing.is_none() {
                    if let Some(first) = free_slots.into_iter().next() {
                        let sk = self.slot_insert(first);
                        self.slot_add_job(sk, *job);
                    }
                }
            } else {
                for (i, free) in free_slots.into_iter().enumerate() {
                    let target = match existing {
                        Some(sk) if i == 0 => Target::Booked(sk),
                        _ => Target::Free(free),
                    };
                    if self.set_free_referee_for(&referee_list, target, *job) {
                        break;
                    }
                }
            }
        }

        self.fix_affinities(&jobs);
        self.preserve_piste(&jobs, &sticky);

        if let Some(start) = jobs.first().and_then(|j| self.jobs.get(j)).and_then(|j| j.slot).and_then(|s| self.slots.get(&s)).map(|s| s.start) {
            self.timeline.set_cursor_time(start);
        }
        self.refresh_workload(&weapon);
        self.refresh_availability();
        self.batch_mute(c, b, false);
        self.batch_spread(c, b);
        self.redraw();
    }

    /// `Hall::GetFreePisteSlots` : créneaux libres de toutes les pistes (ou
    /// des pistes sélectionnées) à partir du curseur, triés par heure.
    pub fn free_piste_slots(&self, duration: Span, all_pistes: bool) -> Vec<Slot> {
        let when = self.timeline.cursor_time(true);
        let pistes: Vec<PisteKey> = if all_pistes { self.pistes.iter().map(|p| p.key).collect() } else { self.selected.clone() };
        let mut slots: Vec<Slot> = pistes.into_iter().flat_map(|p| self.piste_free_slots(p, when, duration)).collect();
        slots.sort_by(Slot::compare_availability);
        slots
    }

    /// `Hall::CompareReferee` : affinités (par titre) puis charge de travail.
    fn compare_referee(&self, a: u32, b: u32, job: JobKey) -> std::cmp::Ordering {
        let (Some(ra), Some(rb)) = (self.referees.get(a), self.referees.get(b)) else { return std::cmp::Ordering::Equal };
        let fencers = self.jobs.get(&job).map(|j| j.fencers.as_slice()).unwrap_or(&[]);
        let n = crate::affinities::TITLES.len();
        let mut ka = vec![0u32; n];
        let mut kb = vec![0u32; n];
        for f in fencers {
            let ka_bits = ra.affinities.kinship_with(Some(&f.affinities), &self.affinities);
            let kb_bits = rb.affinities.kinship_with(Some(&f.affinities), &self.affinities);
            for i in 0..n {
                if ka_bits & (1 << i) != 0 {
                    ka[i] += 1;
                }
                if kb_bits & (1 << i) != 0 {
                    kb[i] += 1;
                }
            }
        }
        for i in 0..n {
            if ka[i] != kb[i] {
                return ka[i].cmp(&kb[i]);
            }
        }
        ra.workload.cmp(&rb.workload)
    }

    /// `Hall::SetFreeRefereeFor` : premier arbitre (le moins apparenté puis le
    /// moins chargé) libre pour ce créneau ; y place le match et l'arbitre.
    fn set_free_referee_for(&mut self, referees: &[u32], target: Target, job: JobKey) -> bool {
        let duration = self.jobs.get(&job).map(|j| j.duration()).unwrap_or(0);
        let mut sorted = referees.to_vec();
        sorted.sort_by(|a, b| self.compare_referee(*a, *b, job));
        let slot = match &target {
            Target::Booked(sk) => match self.slots.get(sk) {
                Some(s) => s.clone(),
                None => return false,
            },
            Target::Free(s) => s.clone(),
        };
        for r in sorted {
            if self.referee_available_slot(r, &slot, duration).is_some() {
                let sk = match target {
                    Target::Booked(sk) => sk,
                    Target::Free(s) => self.slot_insert(s),
                };
                // `TailWith` : le créneau prend la durée du match.
                self.slot_set_duration(sk, duration);
                self.slot_add_job(sk, job);
                self.slot_add_referee(sk, r);
                return true;
            }
        }
        false
    }

    /// `Hall::FixAffinities` : échange les arbitres de deux créneaux simultanés
    /// si cela réduit les parentés.
    fn fix_affinities(&mut self, jobs: &[JobKey]) {
        if !self.console.fix_affinities {
            return;
        }
        for (ia, ja) in jobs.iter().enumerate() {
            let Some(sa) = self.jobs.get(ja).and_then(|j| j.slot) else { continue };
            let refs_a = self.slots.get(&sa).map(|s| s.referees.clone()).unwrap_or_default();
            if refs_a.len() != 1 {
                continue;
            }
            let ra = refs_a[0];
            for (ib, jb) in jobs.iter().enumerate() {
                if ia == ib {
                    continue;
                }
                let Some(sb) = self.jobs.get(jb).and_then(|j| j.slot) else { continue };
                let (Some(slot_a), Some(slot_b)) = (self.slots.get(&sa).cloned(), self.slots.get(&sb).cloned()) else { continue };
                if !slot_a.equals(&slot_b) || slot_b.referees.len() != 1 {
                    continue;
                }
                let rb = slot_b.referees[0];
                let ka = self.jobs.get(ja).map(|j| j.kinship).unwrap_or(0) as i64;
                let kb = self.jobs.get(jb).map(|j| j.kinship).unwrap_or(0) as i64;
                let delta_a = self.job_kinship_with(*ja, rb) as i64 - ka;
                let delta_b = self.job_kinship_with(*jb, ra) as i64 - kb;
                if delta_a + delta_b < 0 {
                    self.slot_remove_referee(sa, ra);
                    self.slot_remove_referee(sb, rb);
                    let (Some(na), Some(nb)) = (self.slots.get(&sa).cloned(), self.slots.get(&sb).cloned()) else { break };
                    if self.referee_is_available(ra, &nb, nb.duration) && self.referee_is_available(rb, &na, na.duration) {
                        self.slot_add_referee(sa, rb);
                        self.slot_add_referee(sb, ra);
                    } else {
                        self.slot_add_referee(sa, ra);
                        self.slot_add_referee(sb, rb);
                    }
                    break;
                }
            }
        }
    }

    /// `Hall::PreservePiste` : un arbitre enchaîne sur la piste de son match
    /// précédent quand c'est possible.
    fn preserve_piste(&mut self, jobs: &[JobKey], sticky: &[SlotKey]) {
        if !self.console.preserve_piste {
            return;
        }
        let mut sorted: Vec<JobKey> = jobs.to_vec();
        sorted.sort_by(|a, b| {
            let sa = self.jobs.get(a).and_then(|j| j.slot).and_then(|s| self.slots.get(&s)).map(|s| s.start);
            let sb = self.jobs.get(b).and_then(|j| j.slot).and_then(|s| self.slots.get(&s)).map(|s| s.start);
            match (sa, sb) {
                (Some(x), Some(y)) => x.cmp(&y),
                _ => std::cmp::Ordering::Equal,
            }
        });
        let mut swap_list: Vec<SlotKey> = Vec::new();
        let mut first: Option<SlotKey> = None;
        for j in sorted {
            let Some(sk) = self.jobs.get(&j).and_then(|x| x.slot) else { continue };
            let Some(s) = self.slots.get(&sk) else { continue };
            if s.referees.is_empty() {
                continue;
            }
            match first {
                None => first = Some(sk),
                Some(f) => {
                    if !sticky.contains(&sk) {
                        let fs = self.slots.get(&f).map(|x| x.start);
                        if fs != Some(s.start) && !swap_list.contains(&sk) {
                            swap_list.push(sk);
                        }
                    }
                }
            }
        }

        let mut swappable = swap_list.clone();
        while let Some(&a) = swap_list.first() {
            let referee_a = self.slots.get(&a).and_then(|s| s.referees.first().copied());
            if let Some(referee_a) = referee_a {
                if let Some(before) = self.referee_slot_just_before(referee_a, a) {
                    let needed = self.slots.get(&before).and_then(|s| s.piste);
                    let (current, start, duration) = match self.slots.get(&a) {
                        Some(s) => (s.piste, s.start, s.duration),
                        None => (None, time::now_local(), 0),
                    };
                    if let Some(needed) = needed.filter(|n| Some(*n) != current) {
                        if let Some(free) = self.piste_free_slot(needed, start, duration) {
                            let fk = self.slot_insert(free);
                            self.slot_swap(a, fk);
                        } else if let Some(&b) = swappable.iter().find(|b| {
                            **b != a
                                && self.slots.get(b).and_then(|s| s.piste) == Some(needed)
                                && self.slots.get(b).zip(self.slots.get(&a)).map(|(x, y)| x.equals(y)).unwrap_or(false)
                        }) {
                            self.slot_swap(a, b);
                            swap_list.retain(|x| *x != b);
                            swappable.retain(|x| *x != b);
                        }
                    }
                }
            }
            swap_list.retain(|x| *x != a);
        }
        self.gc_slots();
    }

    /// `Hall::OnBatchAssignmentCancel` : retire tous les matchs du lot des pistes.
    pub fn batch_assignment_cancel(&mut self, c: u32, b: u32) {
        let Some(weapon) = self.competition(c).map(|x| x.weapon.clone()) else { return };
        self.batch_mute(c, b, true);
        let jobs: Vec<JobKey> = self.jobs.iter().filter(|(_, j)| j.competition == c && j.batch == b).map(|(k, _)| *k).collect();
        for job in jobs {
            if let Some(sk) = self.jobs.get(&job).and_then(|j| j.slot) {
                self.slot_remove_job(sk, job);
                self.job_reset_roadmap(job);
            }
        }
        self.gc_slots();
        self.redraw();
        self.refresh_workload(&weapon);
        self.events.push(UiEvent::ExposeWeapon(weapon));
        self.batch_mute(c, b, false);
        self.batch_recall(c, b);
    }

    /// Retire un match (ou une poule) de sa piste, avec ses arbitres
    /// (fiche du match, double-clic sur la piste).
    pub fn job_take_off(&mut self, job: JobKey) {
        let Some((sk, c)) = self.jobs.get(&job).and_then(|j| Some((j.slot?, j.competition))) else { return };
        let weapon = self.competition(c).map(|x| x.weapon.clone());
        self.slot_remove_job(sk, job);
        self.job_reset_roadmap(job);
        self.gc_slots();
        self.redraw();
        if let Some(w) = weapon {
            self.refresh_workload(&w);
            self.events.push(UiEvent::ExposeWeapon(w));
        }
    }

    /// Retire les arbitres d'un match ; le match reste sur sa piste.
    pub fn job_remove_referees(&mut self, job: JobKey) {
        let Some((sk, c)) = self.jobs.get(&job).and_then(|j| Some((j.slot?, j.competition))) else { return };
        while let Some(r) = self.slots.get(&sk).and_then(|s| s.referees.first().copied()) {
            self.slot_remove_referee(sk, r);
        }
        self.redraw();
        if let Some(w) = self.competition(c).map(|x| x.weapon.clone()) {
            self.refresh_workload(&w);
            self.events.push(UiEvent::ExposeWeapon(w));
        }
    }

    /// `Batch::OnExtension` : durée doublée ou triplée des matchs en attente.
    pub fn batch_extension(&mut self, c: u32, b: u32, choice: u8) {
        self.batch_assignment_cancel(c, b);
        let factor = match choice {
            1 => 2,
            2 => 3,
            _ => 1,
        };
        let pending = self.batch(c, b).map(|x| x.pending.clone()).unwrap_or_default();
        for j in pending {
            if let Some(job) = self.jobs.get_mut(&j) {
                job.extend_duration(factor);
            }
        }
        if let Some(batch) = self.batch_mut(c, b) {
            batch.extension_choice = choice;
        }
    }

    // --------------------------------------------------------------------------------
    // Déplacement d'un match (tableau des matchs → piste)

    /// `Hall::OnJobMove` : le match quitte son créneau et suit la souris.
    pub fn start_moving_job(&mut self, job: JobKey) {
        self.cancel_selection();
        let Some(sk) = self.jobs.get(&job).and_then(|j| j.slot) else { return };
        let referees = self.slots.get(&sk).map(|s| s.referees.clone()).unwrap_or_default();
        let additional_pistes = self.slots.get(&sk).map(|s| s.additional_pistes.clone()).unwrap_or_default();
        self.floating = Some(FloatingJob { job, referees, additional_pistes });
        self.slot_remove_job(sk, job);
        self.gc_slots();
        if let Some(c) = self.jobs.get(&job).map(|j| j.competition) {
            if let Some(w) = self.competition(c).map(|x| x.weapon.clone()) {
                self.refresh_workload(&w);
            }
        }
        self.toolbar_enabled = false;
        self.redraw_scheduled = false;
        self.events.push(UiEvent::Blur);
        self.on_timeline_cursor_moved();
    }

    /// `Hall::StopMovingJob`.
    pub fn stop_moving_job(&mut self) {
        if self.floating.take().is_some() {
            self.events.push(UiEvent::UnBlur);
            self.toolbar_enabled = true;
            self.on_timeline_cursor_moved();
        }
        self.redraw_scheduled = true;
    }

    /// `Hall::OnTimelineCursorMoved` : les pistes affichent l'heure du
    /// curseur ; pendant un déplacement, celles qui ne peuvent pas recevoir
    /// le match sont voilées.
    pub fn on_timeline_cursor_moved(&mut self) {
        let cursor = self.timeline.cursor_time(false);
        let keys: Vec<PisteKey> = self.pistes.iter().map(|p| p.key).collect();
        for k in keys {
            let mut blur = false;
            if let Some(f) = self.floating.clone() {
                if self.piste_slot_at(k, cursor).is_some() {
                    blur = true;
                } else {
                    let duration = self.jobs.get(&f.job).map(|j| j.duration()).unwrap_or(0);
                    match self.piste_free_slot(k, cursor, duration) {
                        None => blur = true,
                        Some(dest) => {
                            if f.referees.iter().any(|r| !self.referee_is_available(*r, &dest, duration)) {
                                blur = true;
                            }
                        }
                    }
                }
                let duration = self.jobs.get(&f.job).map(|j| j.duration()).unwrap_or(0);
                if f.additional_pistes.contains(&k) || f.additional_pistes.iter().any(|p| self.piste_free_slot(*p, cursor, duration).is_none()) {
                    blur = true;
                }
            }
            if let Some(p) = self.piste_mut(k) {
                p.display_time = cursor;
                p.blurred = blur;
            }
        }
    }

    /// `Hall::OnTimelineCursorChanged`.
    pub fn on_timeline_cursor_changed(&mut self) {
        self.refresh_availability();
    }

    /// `Hall::OnNewWarningPolicy` : validité des affinités puis état des matchs.
    pub fn on_new_warning_policy(&mut self) {
        let mut all: Vec<(u32, u32, JobKey)> = Vec::new();
        for c in &self.competitions {
            for b in &c.batches {
                for j in &b.scheduled {
                    all.push((c.id, b.id, *j));
                }
            }
        }
        for (c, b, j) in all {
            self.job_refresh_status(j);
            self.batch_on_new_affinities_rule(c, b, j);
        }
    }

    /// Validité d'une affinité (cases « club », « ligue », « pays »).
    pub fn set_warning_validity(&mut self, title: &str, valid: bool) {
        self.affinities.set_validity(title, valid);
        self.on_new_warning_policy();
    }

    /// `Hall::AlterClock` (si la console l'autorise).
    pub fn alter_clock(&mut self) {
        if self.console.clock_alterable {
            let cursor = self.timeline.cursor_time(true);
            let mut out = Vec::new();
            self.clock.set(cursor, &mut out);
            self.outbox.extend(out);
            self.redraw();
        }
    }
}

/// Créneau visé par `set_free_referee_for` : déjà réservé (le match y est
/// déjà) ou libre (à réserver).
enum Target {
    Booked(SlotKey),
    Free(Slot),
}
