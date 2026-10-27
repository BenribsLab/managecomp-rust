//! Tournoi : l'ensemble des compétitions ouvertes (port de
//! `supervisor/tournament.cpp`, partie logique).
//!
//! Le tournoi ouvre les fichiers (compétition, liste de tireurs, archive
//! ZIP, AskFred), garde les préférences utilisateur, les dossiers de
//! sauvegarde et la liste des fichiers récents. Les messages réseau sont
//! aiguillés par l'application, qui connaît à la fois le tournoi et le réseau.

pub mod user_config;

use std::path::{Path, PathBuf};

use crate::contest::Contest;
use crate::formats::archive::{self, FileKind};
use crate::formats::askfred::reader::AskFredFile;
use crate::tournament::user_config::UserConfig;
use crate::{Error, Result};

/// Identifiant stable d'une compétition ouverte (l'onglet de l'interface).
pub type ContestId = u32;

/// Nombre de fichiers récents mémorisés.
pub const MAX_RECENT_FILES: usize = 10;

/// Résultat de l'ouverture d'un fichier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opened {
    /// Compétitions créées (une par épreuve d'un fichier AskFred).
    pub contests: Vec<ContestId>,
    /// Liste de tireurs importée : l'interface doit demander les propriétés
    /// de la compétition, puis un nom de fichier (`AskForSettings`).
    pub ask_for_settings: bool,
    /// Fichier AskFred : informer l'utilisateur qu'il doit vérifier et
    /// enregistrer chaque épreuve.
    pub askfred: bool,
    /// Compétition lue dans un fichier qui n'est pas un `.cotcot` : il faut
    /// choisir un nom pour l'enregistrer.
    pub needs_file_name: bool,
}

pub struct Tournament {
    contests: Vec<(ContestId, Contest)>,
    next_id: ContestId,
    pub config: UserConfig,
    config_path: Option<PathBuf>,
    backup_active: bool,
    /// Dossier des exemples (`Exemples`).
    pub examples_dir: Option<PathBuf>,
    /// Dossier où le serveur web publie une copie des compétitions.
    pub www_dir: Option<PathBuf>,
}

impl Tournament {
    pub fn new(config: UserConfig) -> Tournament {
        let backup_active = config.get_string("Tournament", "backup_location").is_some();
        Tournament {
            contests: Vec::new(),
            next_id: 1,
            config,
            config_path: None,
            backup_active,
            examples_dir: None,
            www_dir: None,
        }
    }

    /// Charge les préférences depuis leur fichier habituel.
    pub fn with_default_config() -> Tournament {
        let path = UserConfig::default_path();
        let mut t = Tournament::new(UserConfig::load(&path));
        t.config_path = Some(path);
        t
    }

    /// Enregistre les préférences.
    pub fn save_config(&self) -> std::io::Result<()> {
        self.config.save(self.config_path.as_deref())
    }

    // ----------------------------------------------------------------------
    // Compétitions ouvertes

    /// `Manage` : la plus récente est en tête.
    pub fn manage(&mut self, contest: Contest) -> ContestId {
        let id = self.next_id;
        self.next_id += 1;
        self.contests.insert(0, (id, contest));
        id
    }

    /// Nouvelle compétition vide avec ses phases par défaut (`OnNew`, choix « vide »).
    pub fn new_contest(&mut self) -> ContestId {
        let contest = Contest::new_with_default_stages(Some(&mut self.config));
        self.manage(contest)
    }

    /// Nouvel entraînement des jeunes : formule appel → création des
    /// groupes → poules → tableaux → classement général, dans une seule
    /// compétition.
    pub fn new_youth_training(&mut self) -> ContestId {
        let mut contest = Contest::new_with_default_stages(Some(&mut self.config));
        contest.with_schedule(|schedule, ctx| {
            schedule.add_kind(ctx, crate::stage::StageKind::YouthTraining, Some(0));
        });
        self.manage(contest)
    }

    /// Prépare les compétitions créées au franchissement d'un point de
    /// scission : une par division utilisée. Cette méthode n'avance pas la
    /// source : l'interface verrouille ensuite la phase et affiche les onglets
    /// des nouvelles compétitions.
    pub fn split_current_contest(&mut self, source: ContestId) -> Result<Vec<ContestId>> {
        let at = self.contests.iter().position(|(id, _)| *id == source)
            .ok_or_else(|| Error::Format("compétition source introuvable".into()))?;
        let source = &self.contests[at].1;
        let index = source.schedule.current_index();
        if source.schedule.current().is_none_or(|s| s.base().locked) {
            return Err(Error::Format("le point de scission n'est pas modifiable".into()));
        }
        let divisions = source.split_divisions(index);
        let mut targets = Vec::new();
        for division in divisions {
            let source = &self.contests[at].1;
            targets.push(source.duplicate_split(index, division, Some(&mut self.config))?);
        }
        Ok(targets.into_iter().map(|t| self.manage(t)).collect())
    }

    /// `OnContestDeleted`.
    pub fn close(&mut self, id: ContestId) -> Option<Contest> {
        let i = self.contests.iter().position(|(c, _)| *c == id)?;
        Some(self.contests.remove(i).1)
    }

    pub fn contest(&self, id: ContestId) -> Option<&Contest> {
        self.contests.iter().find(|(c, _)| *c == id).map(|(_, c)| c)
    }

    pub fn contest_mut(&mut self, id: ContestId) -> Option<&mut Contest> {
        self.contests.iter_mut().find(|(c, _)| *c == id).map(|(_, c)| c)
    }

    pub fn ids(&self) -> Vec<ContestId> {
        self.contests.iter().map(|(id, _)| *id).collect()
    }

    pub fn len(&self) -> usize {
        self.contests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contests.is_empty()
    }

    /// `GetContest (netid)`.
    pub fn contest_by_net_id(&self, net_id: u32) -> Option<ContestId> {
        self.contests.iter().find(|(_, c)| c.net_id == net_id).map(|(id, _)| *id)
    }

    /// `GetContest (filename)`.
    pub fn contest_by_filename(&self, path: &Path) -> Option<ContestId> {
        self.contests.iter().find(|(_, c)| c.filename.as_deref() == Some(path)).map(|(id, _)| *id)
    }

    // ----------------------------------------------------------------------
    // Ouverture de fichiers

    /// `OpenUriContest` : ouvre un fichier selon son type.
    pub fn open(&mut self, path: &Path) -> Result<Opened> {
        if let Some(dir) = path.parent() {
            self.config.set_string("Competiton", "default_dir_name", &dir.to_string_lossy());
        }
        let name = path.to_string_lossy().to_string();
        let mut opened = Opened::default();
        match archive::file_kind(&name) {
            FileKind::AskFred => {
                let file = AskFredFile::parse_file(path)?;
                for event in &file.events {
                    let mut contest = Contest::new(Some(&mut self.config), false);
                    contest.load_askfred(&file, event);
                    opened.contests.push(self.manage(contest));
                }
                opened.askfred = true;
            }
            FileKind::Zip => {
                let bytes = std::fs::read(path)?;
                // Seul le premier fichier XML de l'archive est ouvert.
                let entries = archive::xml_entries(&bytes)?;
                let Some((_, content)) = entries.into_iter().next() else {
                    return Err(Error::Format(format!("aucune compétition XML dans {name}")));
                };
                let text = crate::formats::encoding::decode_xml(&content)?;
                let mut contest = Contest::new(Some(&mut self.config), false);
                contest.load_xml_str(&text)?;
                opened.needs_file_name = true;
                opened.contests.push(self.manage(contest));
            }
            FileKind::Contest => {
                let mut contest = Contest::new(Some(&mut self.config), false);
                contest.load_file(path)?;
                opened.needs_file_name = contest.filename.is_none();
                self.add_recent(path);
                opened.contests.push(self.manage(contest));
            }
            FileKind::Fff | FileKind::Csv => {
                let mut contest = Contest::new(Some(&mut self.config), false);
                contest.load_fencer_file(path)?;
                opened.ask_for_settings = true;
                opened.contests.push(self.manage(contest));
            }
            FileKind::Unknown => {
                return Err(Error::Format(format!("type de fichier non reconnu : {name}")));
            }
        }
        Ok(opened)
    }

    /// `OnOpenTemplate` : contenu du modèle CSV de liste de tireurs.
    pub fn csv_template(&self) -> String {
        crate::formats::csv::template()
    }

    /// Fichiers du dossier des exemples (`OnOpenExample`).
    pub fn examples(&self) -> Vec<PathBuf> {
        let Some(dir) = &self.examples_dir else { return Vec::new() };
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect())
            .unwrap_or_default();
        files.sort();
        files
    }

    // ----------------------------------------------------------------------
    // Fichiers récents

    /// Seuls les fichiers de compétition (`.cotcot`) sont retenus : les
    /// listes importées (FFF, CSV, arbitres…) n'en font pas partie.
    pub fn add_recent(&mut self, path: &Path) {
        if !is_contest_file(path) {
            return;
        }
        let path = path.to_string_lossy().to_string();
        let mut list = self.recent_raw();
        list.retain(|p| *p != path);
        list.insert(0, path);
        list.truncate(MAX_RECENT_FILES);
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        self.config.set_string_list("Tournament", "recent_files", &refs);
    }

    /// Retire un fichier de la liste des récents (le fichier est conservé).
    pub fn remove_recent(&mut self, path: &Path) {
        let path = path.to_string_lossy().to_string();
        let mut list = self.recent_raw();
        list.retain(|p| *p != path);
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        self.config.set_string_list("Tournament", "recent_files", &refs);
    }

    fn recent_raw(&self) -> Vec<String> {
        self.config.get_string_list("Tournament", "recent_files").unwrap_or_default()
    }

    /// `RecentFileExists` : seuls les fichiers qui existent encore.
    pub fn recent_files(&self) -> Vec<PathBuf> {
        self.recent_raw().into_iter().map(PathBuf::from).filter(|p| p.is_file() && is_contest_file(p)).collect()
    }

    // ----------------------------------------------------------------------
    // Sauvegardes

    /// `SetBackupLocation` : mémorise le dossier de sauvegarde et l'active.
    pub fn set_backup_location(&mut self, dir: &Path) {
        self.config.set_string("Tournament", "backup_location", &dir.to_string_lossy());
        self.backup_active = true;
    }

    pub fn set_backup_active(&mut self, active: bool) {
        self.backup_active = active;
    }

    pub fn backup_active(&self) -> bool {
        self.backup_active
    }

    /// Dossier mémorisé, même si la sauvegarde est désactivée.
    pub fn last_backup_location(&self) -> Option<PathBuf> {
        self.config.get_string("Tournament", "backup_location").map(PathBuf::from)
    }

    /// `GetBackupLocation` : dossier de sauvegarde si elle est active et
    /// que le dossier existe.
    pub fn backup_location(&self) -> Option<PathBuf> {
        if !self.backup_active {
            return None;
        }
        self.last_backup_location().filter(|d| d.is_dir())
    }

    /// Enregistre une compétition avec ses copies web et de sauvegarde.
    pub fn save_contest(&mut self, id: ContestId) -> Result<()> {
        let backup = self.backup_location();
        let www = self.www_dir.clone();
        match self.contest_mut(id) {
            Some(c) => c.save(www.as_deref(), backup.as_deref()),
            None => Ok(()),
        }
    }

    /// `OnSave` : enregistre toutes les compétitions.
    pub fn save_all(&mut self) -> Result<()> {
        for id in self.ids() {
            self.save_contest(id)?;
        }
        Ok(())
    }

    /// Archive une compétition à chaque changement de phase.
    pub fn archive_contest(&mut self, id: ContestId, title: &str) -> Result<()> {
        let backup = self.backup_location();
        let www = self.www_dir.clone();
        match self.contest_mut(id) {
            Some(c) => c.archive(title, www.as_deref(), backup.as_deref()),
            None => Ok(()),
        }
    }

    /// `OnReload` : relit les fichiers de tireurs sources de chaque compétition.
    pub fn reload_fencers(&mut self) {
        for (_, contest) in self.contests.iter_mut() {
            contest.with_schedule(|schedule, ctx| {
                if let Some(checkin) = schedule.checkin_mut() {
                    let _ = checkin.reload_fencers(ctx);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_are_unique_and_bounded() {
        let mut t = Tournament::new(UserConfig::new());
        for i in 0..15 {
            t.add_recent(Path::new(&format!("f{i}.cotcot")));
        }
        t.add_recent(Path::new("f14.cotcot"));
        let raw = t.recent_raw();
        assert_eq!(raw.len(), MAX_RECENT_FILES);
        assert_eq!(raw[0], "f14.cotcot");
        assert_eq!(raw.iter().filter(|p| *p == "f14.cotcot").count(), 1);
    }

    #[test]
    fn new_contest_is_tracked() {
        let mut t = Tournament::new(UserConfig::new());
        let id = t.new_contest();
        assert_eq!(t.len(), 1);
        let net = t.contest(id).unwrap().net_id;
        assert_eq!(t.contest_by_net_id(net), Some(id));
        assert!(t.close(id).is_some());
        assert!(t.is_empty());
    }
}

/// Fichier de compétition BellePoule (`.cotcot`).
fn is_contest_file(path: &Path) -> bool {
    path.extension().map(|e| e.eq_ignore_ascii_case("cotcot")).unwrap_or(false)
}
