//! Arbitres engagés et listes par arme (port de `referee_pool.cpp`,
//! `enlisted_referee.cpp`, et des parties `RefereesList` / `Checkin` /
//! `Referee` utiles au Marshaller).

use std::collections::BTreeMap;
use std::path::Path;

use bp_core::attribute::{self, Value};
use bp_core::player::AttrId;
use bp_core::{Player, PlayerClass};
use bp_net::Message;
use sha1::{Digest, Sha1};

use crate::affinities::Affinities;
use crate::color::Rgb;
use crate::hall::Hall;
use crate::model::{spread, EnlistedReferee, NetCommand, RefereeList};
use crate::slot::{Slot, SlotKey};
use crate::time::Span;
use crate::weapon::WEAPONS;

/// Ensemble des arbitres (`RefereePool`).
#[derive(Debug, Clone)]
pub struct RefereePool {
    pub referees: BTreeMap<u32, EnlistedReferee>,
    /// Une liste par arme, dans l'ordre de `WEAPONS` (onglets).
    pub lists: Vec<RefereeList>,
    /// Les colis `BellePoule2D::Referee` ont été dévoilés (`Disclose`).
    pub disclosed: bool,
    /// Clé de chiffrement du réseau (`Ring::GetCryptorKey`).
    pub cryptor_key: Option<String>,
}

impl Default for RefereePool {
    fn default() -> Self {
        Self::new()
    }
}

impl RefereePool {
    pub fn new() -> Self {
        RefereePool {
            referees: BTreeMap::new(),
            lists: WEAPONS.iter().map(|w| RefereeList { weapon: w, members: Vec::new(), collapsed: true }).collect(),
            disclosed: false,
            cryptor_key: None,
        }
    }

    /// `GetListOf` (comparaison insensible à la casse).
    pub fn list_index(&self, weapon: &str) -> Option<usize> {
        self.lists.iter().position(|l| l.weapon.xml.eq_ignore_ascii_case(weapon))
    }

    pub fn get(&self, reference: u32) -> Option<&EnlistedReferee> {
        self.referees.get(&reference)
    }

    /// Membres présents / absents d'une liste (`TallyCounter`).
    pub fn tally(&self, list: usize) -> (usize, usize) {
        let members = &self.lists[list].members;
        let present = members.iter().filter(|r| self.referees.get(r).map(|x| x.player.attending()).unwrap_or(false)).count();
        (present, members.len() - present)
    }

    /// `WeaponHasReferees`.
    pub fn weapon_has_referees(&self, weapon: &str) -> bool {
        self.list_index(weapon).map(|i| self.tally(i).0 > 0).unwrap_or(false)
    }

    /// L'arbitre est-il disponible pour l'arme de la liste (lettre en majuscule) ?
    pub fn available_for_weapon(&self, reference: u32, list: usize) -> bool {
        let code = self.lists[list].weapon.xml.chars().next().unwrap_or(' ');
        self.referees
            .get(&reference)
            .and_then(|r| r.player.str_attr("weapon").map(str::to_string))
            .and_then(|w| w.chars().find(|c| c.to_ascii_uppercase() == code))
            .map(|c| c.is_ascii_uppercase())
            .unwrap_or(true)
    }

    /// XML complet d'un arbitre (`Player::FeedParcel` + `Referee::SaveAttributes`).
    pub fn referee_xml(&self, referee: &EnlistedReferee) -> String {
        let mut player = referee.player.clone();
        if let (Some(password), Some(key)) = (player.str_attr("password").map(str::to_string), self.cryptor_key.as_deref()) {
            let cypher = crate::crypto::encrypt(&password, key, &crate::crypto::zero_iv_b64());
            player.set_str(&AttrId::global("cyphered_password"), &cypher);
        }
        let mut w = bp_core::xml::XmlWriter::new().without_indent();
        player.save(&mut w, true);
        w.finish()
    }
}

/// `Referee::GiveAnId` : identifiant tiré du SHA1 « nom + prénom » et
/// déchiffrement du mot de passe éventuel.
pub fn give_an_id(player: &mut Player, cryptor_key: Option<&str>) {
    if player.attr("ref").is_none() {
        let mut h = Sha1::new();
        h.update(player.name().as_bytes());
        h.update(player.first_name().as_bytes());
        let hex = format!("{:x}", h.finalize());
        let reference = u32::from_str_radix(&hex[..4], 16).unwrap_or(0);
        player.set_ref(reference);
    }
    if let (Some(cypher), Some(key)) = (player.str_attr("cyphered_password").map(str::to_string), cryptor_key) {
        if let Some(password) = crate::crypto::decrypt(&cypher, &crate::crypto::zero_iv_b64(), key) {
            if password.len() == 256 / 8 {
                player.set_str(&AttrId::global("password"), &password);
            }
        }
    }
}

/// Nouvel arbitre engagé (`EnlistedReferee::EnlistedReferee`).
pub fn enlist(mut player: Player) -> EnlistedReferee {
    player.class = PlayerClass::Referee;
    if player.attr("connection").is_none() {
        player.set_str(&AttrId::global("connection"), "Manual");
    }
    player.set_uint(&AttrId::global("workload_rate"), 0);
    if player.attr("attending").is_none() {
        player.set(&AttrId::global("attending"), Value::Bool(false));
    }
    let affinities = Affinities::new(&player);
    EnlistedReferee {
        player,
        slots: Vec::new(),
        workload: 0,
        affinities,
        cell_color: None,
        parcel: None,
        wifi_key: None,
    }
}

/// Valeurs localisées (`localized_data/<pays>/<dossier>/index.txt`) :
/// nom affiché pour un code numérique.
pub fn localized_name(dir: &Path, code: u32) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("index.txt")).ok()?;
    for line in text.lines() {
        let tokens: Vec<&str> = line.split(';').collect();
        if tokens.len() == 4 && tokens[3].split('-').any(|c| c.trim().parse::<u32>().ok() == Some(code)) {
            return Some(tokens[0].to_string());
        }
    }
    None
}

/// `Checkin::GuessPlayerOrganization` : ligue / région d'après la licence FFE.
pub fn guess_organization(player: &mut Player, organization: &str) {
    let french = player.str_attr("country").map(|c| c.eq_ignore_ascii_case("FRA")).unwrap_or(false);
    if !french {
        return;
    }
    let Some(licence) = player.str_attr("licence").map(str::to_string) else { return };
    if licence.len() != 12 {
        return;
    }
    let code = attribute::parse_leading_int(&licence[..2]).max(0) as u32;
    let folder = if organization == "league" { "ligues" } else { "regions" };
    let dir = crate::config::resources_dir().join("localized_data").join("fra").join(folder);
    if let Some(name) = localized_name(&dir, code) {
        player.set_str(&AttrId::global(organization), &name);
    }
}

impl Hall {
    // --------------------------------------------------------------------------------
    // Messages et diffusion

    /// Diffuse un arbitre si les colis sont dévoilés (`PlayersList::Update`).
    pub fn referee_update(&mut self, reference: u32) {
        if !self.referees.disclosed {
            return;
        }
        let Some(r) = self.referees.referees.get(&reference) else { return };
        let xml = self.referees.referee_xml(r);
        let r = self.referees.referees.get_mut(&reference).unwrap();
        let parcel = r.parcel.get_or_insert_with(|| Message::new("BellePoule2D::Referee"));
        parcel.set_str("xml", &xml);
        spread(&mut self.outbox, parcel);
    }

    /// `RefereePool::Spread` : dévoile et diffuse tous les arbitres.
    pub fn referees_spread(&mut self) {
        self.referees.disclosed = true;
        let refs: Vec<u32> = self.referees.lists.iter().flat_map(|l| l.members.iter().copied()).collect();
        let mut done = Vec::new();
        for r in refs {
            if !done.contains(&r) {
                done.push(r);
                self.referee_update(r);
            }
        }
    }

    /// `RefereePool::ManageReferee` (`BellePoule::Referee`).
    pub fn manage_referee(&mut self, message: &Message) {
        let Some(xml) = message.get_str("xml") else { return };
        let Ok(doc) = bp_core::xml::parse(&xml) else { return };
        let root = doc.root_element();
        if root.tag_name().name() != "Arbitre" {
            return;
        }
        let mut submitted = Player::new(PlayerClass::Referee);
        submitted.load(root);
        let weapons = submitted.str_attr("weapon").unwrap_or_default().to_string();
        let reference = submitted.reference;
        for w in weapons.chars() {
            let Some(list) = self.referees.list_index(&w.to_string()) else { continue };
            if !self.referees.lists[list].members.contains(&reference) {
                let player = match self.referees.referees.get(&reference) {
                    Some(existing) => existing.player.clone(),
                    None => submitted.clone(),
                };
                self.register_referee(list, player);
            }
        }
    }

    /// `Checkin::RegisterPlayer` + `RefereesList::Add`.
    pub fn register_referee(&mut self, list: usize, mut player: Player) -> u32 {
        guess_organization(&mut player, "league");
        guess_organization(&mut player, "region");
        give_an_id(&mut player, self.referees.cryptor_key.as_deref());
        self.add_referee_to_list(list, player)
    }

    /// `RefereesList::Add` : complète l'attribut `weapon` puis inscrit.
    pub fn add_referee_to_list(&mut self, list: usize, mut player: Player) -> u32 {
        let code = self.referees.lists[list].weapon.xml;
        let weapon_id = AttrId::global("weapon");
        match player.str_attr("weapon").map(str::to_string) {
            Some(w) => {
                if !w.to_ascii_uppercase().contains(code) {
                    player.set_str(&weapon_id, &format!("{w}{code}"));
                }
            }
            None => {
                player.set_str(&weapon_id, code);
            }
        }
        let reference = player.reference;
        match self.referees.referees.get_mut(&reference) {
            Some(existing) => {
                existing.player.update_from(&player);
                existing.affinities = Affinities::new(&existing.player);
            }
            None => {
                self.referees.referees.insert(reference, enlist(player));
            }
        }
        let members = &mut self.referees.lists[list].members;
        if !members.contains(&reference) {
            members.push(reference);
        }
        self.referee_update(reference);
        reference
    }

    /// `RefereePool::ManageHandShake` (`SmartPoule::RefereeHandshake`).
    pub fn manage_handshake(&mut self, message: &Message) {
        let reference = message.get_uint("referee_id");
        let Some(address) = message.get_str("address") else { return };
        if let Some(r) = self.referees.referees.get_mut(&reference) {
            r.player.set_str(&AttrId::global("connection"), "OK");
            r.player.set_str(&AttrId::global("IP"), &address);
            self.referee_update(reference);
        }
    }

    // --------------------------------------------------------------------------------
    // Disponibilité

    /// `EnlistedReferee::GetAvailableSlotFor`.
    pub fn referee_available_slot(&self, reference: u32, slot: &Slot, duration: Span) -> Option<Slot> {
        let r = self.referees.referees.get(&reference)?;
        if !r.player.attending() {
            return None;
        }
        let booked: Vec<&Slot> = r.slots.iter().filter_map(|k| self.slots.get(k)).collect();
        crate::slot::free_slots(None, &booked, slot.start, duration).into_iter().find(|free| slot.can_wrap(free))
    }

    /// `EnlistedReferee::IsAvailableFor`.
    pub fn referee_is_available(&self, reference: u32, slot: &Slot, duration: Span) -> bool {
        self.referee_available_slot(reference, slot, duration).is_some()
    }

    /// `EnlistedReferee::GetSlotJustBefore`.
    pub fn referee_slot_just_before(&self, reference: u32, before: SlotKey) -> Option<SlotKey> {
        let start = self.slots.get(&before)?.start;
        let r = self.referees.referees.get(&reference)?;
        r.slots.iter().copied().find(|k| self.slots.get(k).and_then(|s| s.end) == Some(start))
    }

    /// `RefereePool::RefreshWorkload`.
    pub fn refresh_workload(&mut self, weapon: &str) {
        let Some(list) = self.referees.list_index(weapon) else { return };
        let members = self.referees.lists[list].members.clone();
        let total: i32 = members.iter().filter_map(|r| self.referees.referees.get(r)).map(|r| r.workload).sum();
        let mut changed = Vec::new();
        for m in members {
            if let Some(r) = self.referees.referees.get_mut(&m) {
                let rate = if total != 0 { (r.workload * 100) / total } else { 0 };
                if r.player.set_uint(&AttrId::global("workload_rate"), rate.max(0) as u32) {
                    changed.push(m);
                }
            }
        }
        for m in changed {
            self.referee_update(m);
        }
    }

    /// Recalcule la charge de toutes les armes d'un arbitre.
    pub fn refresh_referee_workloads(&mut self, reference: u32) {
        let weapons = self
            .referees
            .referees
            .get(&reference)
            .and_then(|r| r.player.str_attr("weapon").map(str::to_string))
            .unwrap_or_default();
        for w in weapons.chars() {
            self.refresh_workload(&w.to_string());
        }
    }

    /// `RefereePool::RefreshAvailability` : couleur des arbitres occupés à
    /// l'heure du curseur.
    pub fn refresh_availability(&mut self) {
        let cursor = self.timeline.cursor_time(false);
        let mut colors: BTreeMap<u32, Rgb> = BTreeMap::new();
        for p in &self.pistes {
            let Some(sk) = self.piste_slot_at(p.key, cursor) else { continue };
            let Some(slot) = self.slots.get(&sk) else { continue };
            for r in &slot.referees {
                if !self.referee_is_available(*r, slot, slot.duration) {
                    if let Some(j) = slot.jobs.first().and_then(|j| self.jobs.get(j)) {
                        colors.insert(*r, j.color);
                    }
                }
            }
        }
        for (k, r) in self.referees.referees.iter_mut() {
            r.cell_color = colors.get(k).copied();
        }
    }

    // --------------------------------------------------------------------------------
    // Actions de la liste d'arbitres

    /// `RefereePool::SetRefereeAvailability` : passe la lettre de l'arme en
    /// majuscule (disponible) ou minuscule.
    pub fn set_referee_availability(&mut self, list: usize, selection: &[u32], available: bool) {
        let code = self.referees.lists[list].weapon.xml.chars().next().unwrap_or(' ');
        for r in selection {
            let Some(referee) = self.referees.referees.get_mut(r) else { continue };
            let weapons = referee.player.str_attr("weapon").unwrap_or_default().to_string();
            let mut chars: Vec<char> = weapons.chars().collect();
            let mut changed = false;
            for c in chars.iter_mut() {
                if c.to_ascii_uppercase() == code {
                    if available && c.is_ascii_lowercase() {
                        *c = c.to_ascii_uppercase();
                        changed = true;
                        break;
                    } else if !available && c.is_ascii_uppercase() {
                        *c = c.to_ascii_lowercase();
                        changed = true;
                        break;
                    }
                }
            }
            if changed {
                let w: String = chars.into_iter().collect();
                referee.player.set_str(&AttrId::global("weapon"), &w);
                self.referee_update(*r);
            }
        }
    }

    /// `TogglePlayerAttr ("attending")` : présence d'un arbitre.
    pub fn set_referee_attending(&mut self, reference: u32, attending: bool) {
        let Some(r) = self.referees.referees.get_mut(&reference) else { return };
        if r.player.set(&AttrId::global("attending"), Value::Bool(attending)) {
            r.player.set_str(&AttrId::global("global_status"), if attending { "Q" } else { "F" });
        }
        self.on_referee_updated(reference);
    }

    /// Tous présents / tous absents (`OnToggleAllPlayers`).
    pub fn set_list_attending(&mut self, list: usize, attending: bool) {
        for r in self.referees.lists[list].members.clone() {
            self.set_referee_attending(r, attending);
        }
    }

    /// `Marshaller::OnRefereeUpdated`.
    pub fn on_referee_updated(&mut self, reference: u32) {
        self.referee_update(reference);
        if let Some(r) = self.referees.referees.get_mut(&reference) {
            r.affinities = Affinities::new(&r.player);
        }
        self.on_new_warning_policy();
    }

    /// Validation du formulaire (`Checkin::OnFormEvent` + `GiveAnId`).
    /// `existing` désigne l'arbitre modifié, sinon un arbitre est créé.
    pub fn save_referee_form(&mut self, list: usize, mut player: Player, existing: Option<u32>) -> u32 {
        give_an_id(&mut player, self.referees.cryptor_key.as_deref());
        let reference = match existing.and_then(|e| self.referees.referees.get_mut(&e).map(|r| (e, r))) {
            Some((e, r)) => {
                r.player.update_from(&player);
                e
            }
            None => {
                let status = if player.attending() { "Q" } else { "F" };
                player.set_str(&AttrId::global("global_status"), status);
                self.add_referee_to_list(list, player)
            }
        };
        self.on_referee_updated(reference);
        reference
    }

    /// `RemoveSelection` : retire des arbitres d'une liste ; ceux qui ne sont
    /// plus dans aucune liste quittent leurs créneaux.
    pub fn remove_referees(&mut self, list: usize, selection: &[u32]) {
        self.referees.lists[list].members.retain(|r| !selection.contains(r));
        for r in selection {
            if self.referees.lists.iter().any(|l| l.members.contains(r)) {
                continue;
            }
            let slots = self.referees.referees.get(r).map(|x| x.slots.clone()).unwrap_or_default();
            for s in slots {
                self.slot_remove_referee(s, *r);
            }
            if let Some(mut removed) = self.referees.referees.remove(r) {
                if let Some(p) = removed.parcel.as_mut() {
                    crate::model::recall(&mut self.outbox, p);
                }
            }
        }
        let code = self.referees.lists[list].weapon.xml;
        self.refresh_workload(code);
        self.redraw();
    }

    /// Clé personnelle de chaque arbitre (QR code, PIN), créée au besoin
    /// et transmise aux compétitions. Vrai si une clé a été créée.
    pub fn ensure_referee_keys(&mut self) -> bool {
        let refs: Vec<u32> = self.referees.referees.keys().copied().collect();
        let mut created = false;
        for r in refs {
            let has = self.referees.referees.get(&r).and_then(|x| x.player.str_attr("password")).is_some_and(bp_core::smartpoule::is_referee_key);
            if !has {
                self.referee_wifi_key(r);
                created = true;
            }
        }
        created
    }

    /// Clé personnelle d'un arbitre de la salle (sans la créer).
    pub fn referee_key(&self, reference: u32) -> Option<String> {
        self.referees.referees.get(&reference)?.player.str_attr("password").filter(|k| bp_core::smartpoule::is_referee_key(k)).map(str::to_string)
    }

    /// Nouvelle clé d'un arbitre : nouveau QR code personnel et nouveau PIN.
    pub fn renew_referee_key(&mut self, reference: u32) -> Option<String> {
        let key = bp_core::smartpoule::new_referee_key();
        let r = self.referees.referees.get_mut(&reference)?;
        r.player.set_str(&AttrId::global("password"), &key);
        r.wifi_key = Some(key.clone());
        self.outbox.push(NetCommand::RegisterSecretKey { referee: reference, key: key.clone() });
        self.referee_update(reference);
        Some(key)
    }

    /// Clé du QR code d'un arbitre (`WifiCode::GetKey`) : générée à la
    /// demande et mémorisée dans l'attribut `password`.
    pub fn referee_wifi_key(&mut self, reference: u32) -> Option<String> {
        let r = self.referees.referees.get_mut(&reference)?;
        if r.wifi_key.is_none() {
            if let Some(password) = r.player.str_attr("password").filter(|p| p.len() == 32).map(str::to_string) {
                r.wifi_key = Some(password.clone());
                self.outbox.push(NetCommand::RegisterSecretKey { referee: reference, key: password });
                return Some(r.wifi_key.clone().unwrap_or_default());
            }
            let key = crate::wifi::new_key256();
            r.player.set_str(&AttrId::global("password"), &key);
            r.wifi_key = Some(key.clone());
            self.outbox.push(NetCommand::RegisterSecretKey { referee: reference, key });
            // BellePoule reçoit la clé (chiffrée) pour répondre au smartphone.
            self.referee_update(reference);
        }
        self.referees.referees.get(&reference).and_then(|r| r.wifi_key.clone())
    }

    /// `Marshaller::GetSecretKey` : schéma `x/y/<ref>`.
    pub fn secret_key(&self, scheme: &str) -> Option<String> {
        let tokens: Vec<&str> = scheme.split('/').collect();
        if tokens.len() < 3 {
            return None;
        }
        let reference = attribute::parse_leading_int(tokens[2]).max(0) as u32;
        self.referees.referees.get(&reference)?.wifi_key.clone()
    }

    // --------------------------------------------------------------------------------
    // Import

    /// `Checkin::Import` : fichier FFF ou CSV.
    pub fn import_referees(&mut self, list: usize, path: &Path) -> Result<usize, String> {
        let raw = std::fs::read(path).map_err(|e| e.to_string())?;
        let is_fff = path.extension().map(|e| e.eq_ignore_ascii_case("fff")).unwrap_or(false);
        let players = if is_fff { parse_fff(&raw)? } else { parse_csv(&raw)? };
        let n = players.len();
        for mut p in players {
            give_an_id(&mut p, self.referees.cryptor_key.as_deref());
            self.add_referee_to_list(list, p);
        }
        Ok(n)
    }
}

fn strip_cr(raw: &[u8]) -> Vec<u8> {
    raw.iter().copied().filter(|b| *b != b'\r').collect()
}

/// `Checkin::ImportFFF`.
pub fn parse_fff(raw: &[u8]) -> Result<Vec<Player>, String> {
    let bytes = strip_cr(raw);
    let latin = bytes.windows(8).any(|w| w == b"FFF;WIN;");
    let text = if latin {
        encoding_rs::WINDOWS_1252.decode(&bytes).0.into_owned()
    } else {
        String::from_utf8(bytes).map_err(|_| bp_core::i18n::tr("The imported FFF file has encoding errors."))?
    };
    let lines: Vec<&str> = text.split('\n').collect();
    let mut players = Vec::new();
    if lines.len() < 2 {
        return Ok(players);
    }
    let current_year = chrono::Datelike::year(&chrono::Local::now()) as i64;
    for line in &lines[2..] {
        let fields: Vec<&str> = line.split(';').collect();
        let Some(general) = fields.first() else { continue };
        let mut p = Player::new(PlayerClass::Referee);
        let tokens: Vec<&str> = general.split(',').collect();
        let set = |p: &mut Player, name: &str, v: &str| {
            p.set_str(&AttrId::global(name), v);
        };
        if let Some(v) = tokens.first() {
            set(&mut p, "name", v);
        }
        if let Some(v) = tokens.get(1) {
            set(&mut p, "first_name", v);
        }
        if let Some(d) = tokens.get(2) {
            let parts: Vec<&str> = d.split('/').collect();
            if parts.len() >= 3 {
                let mut year = attribute::parse_leading_int(parts[2]);
                if parts[2].len() <= 2 {
                    let short = year;
                    year = short + current_year - current_year % 100;
                    if short >= current_year % 100 {
                        year -= 100;
                    }
                }
                let day = attribute::parse_leading_int(parts[0]);
                let month = attribute::parse_leading_int(parts[1]);
                set(&mut p, "birth_date", &format!("{day:02}.{month:02}.{year:04}"));
            }
        }
        if let Some(g) = tokens.get(3) {
            let g = if g.starts_with('H') || g.starts_with('h') { format!("M{}", &g[1..]) } else { g.to_string() };
            set(&mut p, "gender", &g);
        }
        if let Some(v) = tokens.get(4) {
            set(&mut p, "country", v);
        }
        if let Some(national) = fields.get(2) {
            let t: Vec<&str> = national.split(',').collect();
            if let Some(v) = t.first() {
                set(&mut p, "licence", v);
            }
            if let Some(v) = t.get(2) {
                set(&mut p, "club", v);
            }
            match t.get(3) {
                Some(v) => set(&mut p, "ranking", v),
                None => {
                    p.set_uint(&AttrId::global("ranking"), 0);
                }
            }
            guess_organization(&mut p, "league");
            guess_organization(&mut p, "region");
        }
        players.push(p);
    }
    Ok(players)
}

/// `Checkin::ImportCSV`.
pub fn parse_csv(raw: &[u8]) -> Result<Vec<Player>, String> {
    let bytes = strip_cr(raw);
    let text = match String::from_utf8(bytes.clone()) {
        Ok(t) => t,
        Err(_) => encoding_rs::WINDOWS_1252.decode(&bytes).0.into_owned(),
    };
    let rows: Vec<&str> = text.split('\n').collect();
    let Some(header) = rows.first() else { return Ok(Vec::new()) };
    let delimiter = if header.contains(';') { ';' } else { ',' };
    let clean = |s: &str| s.replace('"', " ").trim().to_string();
    let columns: Vec<Option<&'static str>> = header
        .split(delimiter)
        .map(|h| {
            attribute::registry().guess_from_user_name(&clean(h)).filter(|d| d.is_csv_ready()).map(|d| d.code_name)
        })
        .collect();
    let mut players = Vec::new();
    for row in &rows[1..] {
        let tokens: Vec<&str> = row.split(delimiter).collect();
        let mut p = Player::new(PlayerClass::Referee);
        let mut has_attribute = false;
        for (c, col) in columns.iter().enumerate() {
            let Some(token) = tokens.get(c) else { break };
            if let Some(code) = col {
                p.set_str(&AttrId::global(code), &clean(token));
                has_attribute = true;
            }
        }
        if has_attribute {
            players.push(p);
        }
    }
    Ok(players)
}
