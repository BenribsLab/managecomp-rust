//! Descripteurs d'attributs et valeurs (port de `common/util/attribute*.cpp`).
//!
//! Le C++ utilise un registre global d'`AttributeDesc` déclaré dans
//! `Application::Prepare`. Ici le registre est construit une fois
//! (`registry()`) et reste immuable, sauf les valeurs discrètes dynamiques
//! (ex. noms d'équipes) qui sont gérées par le modèle de compétition.

use std::cmp::Ordering;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrType {
    Str,
    UInt,
    Int,
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uniqueness {
    Singular,
    NotSingular,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rights {
    Public,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistency {
    Persistent,
    NotPersistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Local,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    LongText,
    ShortText,
    Graphical,
}

/// Fonction de comparaison spécifique (`_compare_func` en C++).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareKind {
    Default,
    /// Dates `jj.mm.aaaa` (ou année seule).
    Date,
    /// Classement : 0 signifie « non classé » et passe en dernier.
    Ranking,
}

/// Valeur discrète d'un attribut (ex. Sexe : `M` → « Male »).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscreteValue {
    pub code: u32,
    /// Image XML (stockée dans les fichiers).
    pub xml: String,
    /// Image utilisateur non traduite (msgid gettext) ; traduire avec `i18n::tr`.
    pub user: String,
    /// Icône (chemin relatif à `resources/`), si `Look::Graphical`.
    pub icon: Option<String>,
}

/// Source de valeurs discrètes chargées depuis `resources/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscreteSource {
    /// `AddDiscreteValueSelector ("countries")` : `resources/countries/`.
    Selector(&'static str),
    /// `AddLocalizedDiscreteValues ("clubs")` : `resources/localized_data/<pays>/`.
    Localized(&'static str),
}

#[derive(Debug, Clone)]
pub struct AttributeDesc {
    pub ty: AttrType,
    pub code_name: &'static str,
    pub xml_name: &'static str,
    /// Libellé non traduit (msgid).
    pub user_name: &'static str,
    pub persistency: Persistency,
    pub scope: Scope,
    pub uniqueness: Uniqueness,
    pub favorite_look: Look,
    pub short_length: u32,
    pub free_value_allowed: bool,
    pub rights: Rights,
    pub compare: CompareKind,
    pub discrete: Vec<DiscreteValue>,
    /// Vrai si l'attribut a des valeurs discrètes dynamiques (ex. `team`).
    pub has_dynamic_discrete: bool,
    pub discrete_source: Option<DiscreteSource>,
    pub sorting_enabled: bool,
    /// Utilisable comme critère de séparation des poules (`AddSwappable`).
    pub swappable: bool,
}

impl AttributeDesc {
    fn new(ty: AttrType, code_name: &'static str, xml_name: &'static str, user_name: &'static str) -> Self {
        AttributeDesc {
            ty,
            code_name,
            xml_name,
            user_name,
            persistency: Persistency::Persistent,
            scope: Scope::Global,
            uniqueness: Uniqueness::Singular,
            favorite_look: Look::LongText,
            short_length: 0,
            free_value_allowed: true,
            rights: Rights::Public,
            compare: CompareKind::Default,
            discrete: Vec::new(),
            has_dynamic_discrete: false,
            discrete_source: None,
            sorting_enabled: false,
            swappable: false,
        }
    }

    pub fn has_discrete_value(&self) -> bool {
        !self.discrete.is_empty() || self.has_dynamic_discrete || self.discrete_source.is_some()
    }

    pub fn discrete_from_xml(&self, xml: &str) -> Option<&DiscreteValue> {
        self.discrete.iter().find(|d| d.xml.eq_ignore_ascii_case(xml))
    }

    pub fn discrete_from_code(&self, code: u32) -> Option<&DiscreteValue> {
        self.discrete.iter().find(|d| d.code == code)
    }

    /// Image XML d'une saisie utilisateur (traduite ou non).
    pub fn xml_image_from_user(&self, user_image: &str) -> String {
        for d in &self.discrete {
            if d.user.eq_ignore_ascii_case(user_image)
                || crate::i18n::tr(&d.user).eq_ignore_ascii_case(user_image)
                || d.xml.eq_ignore_ascii_case(user_image)
            {
                return d.xml.clone();
            }
        }
        user_image.to_string()
    }

    /// Image utilisateur (traduite) d'une valeur XML.
    pub fn user_image(&self, xml: &str, look: Look) -> String {
        match self.discrete_from_xml(xml) {
            Some(d) if look == Look::ShortText => d.xml.clone(),
            Some(d) => crate::i18n::tr(&d.user),
            None => xml.to_string(),
        }
    }

    /// Visible dans les exports CSV (`Application::IsCsvReady`).
    pub fn is_csv_ready(&self) -> bool {
        self.scope == Scope::Global && self.rights == Rights::Public && self.persistency == Persistency::Persistent
    }

    pub fn is_persistent(&self) -> bool {
        self.persistency == Persistency::Persistent
    }
}

/// Valeur typée d'un attribut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    UInt(u32),
    Int(i32),
    Bool(bool),
}

impl Value {
    /// Interprète une chaîne selon le type (équivalent de `SetValue (const gchar *)`).
    pub fn parse(ty: AttrType, s: &str) -> Value {
        let t = s.trim();
        match ty {
            AttrType::Str => Value::Str(s.to_string()),
            AttrType::UInt => Value::UInt(parse_leading_int(t).max(0) as u32),
            AttrType::Int => Value::Int(parse_leading_int(t) as i32),
            AttrType::Bool => Value::Bool(matches!(t, "1" | "true" | "TRUE" | "True" | "oui" | "yes")),
        }
    }

    pub fn from_uint(ty: AttrType, v: u32) -> Value {
        match ty {
            AttrType::Str => Value::Str(v.to_string()),
            AttrType::UInt => Value::UInt(v),
            AttrType::Int => Value::Int(v as i32),
            AttrType::Bool => Value::Bool(v != 0),
        }
    }

    /// Image XML (`GetXmlImage`).
    pub fn xml_image(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::UInt(v) => v.to_string(),
            Value::Int(v) => v.to_string(),
            Value::Bool(b) => if *b { "1".into() } else { "0".into() },
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// `GetUIntValue` : 0 si non numérique.
    pub fn as_uint(&self) -> u32 {
        match self {
            Value::Str(s) => parse_leading_int(s).max(0) as u32,
            Value::UInt(v) => *v,
            Value::Int(v) => *v as u32,
            Value::Bool(b) => *b as u32,
        }
    }

    pub fn as_int(&self) -> i32 {
        match self {
            Value::Str(s) => parse_leading_int(s) as i32,
            Value::UInt(v) => *v as i32,
            Value::Int(v) => *v,
            Value::Bool(b) => *b as i32,
        }
    }

    pub fn as_bool(&self) -> bool {
        self.as_uint() != 0
    }

    /// Comparaison par défaut (`CompareWith`) : texte insensible à la casse
    /// (collation simplifiée), entiers numériquement.
    pub fn compare(&self, other: &Value) -> Ordering {
        match (self, other) {
            (Value::Str(a), Value::Str(b)) => compare_text(a, b),
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (a, b) => a.as_int().cmp(&b.as_int()),
        }
    }
}

/// `g_ascii_strtoll` : lit l'entier en tête de chaîne, 0 sinon.
pub fn parse_leading_int(s: &str) -> i64 {
    let s = s.trim_start();
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    s[..end].parse().unwrap_or(0)
}

/// Comparaison textuelle proche de `g_utf8_collate` sur des chaînes normalisées
/// en minuscules sans accents.
pub fn compare_text(a: &str, b: &str) -> Ordering {
    let ka = collate_key(a);
    let kb = collate_key(b);
    ka.cmp(&kb).then_with(|| a.cmp(b))
}

pub fn collate_key(s: &str) -> String {
    s.chars().map(strip_accent).flat_map(char::to_lowercase).collect()
}

fn strip_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
        'ç' => 'c',
        'Ç' => 'C',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'È' | 'É' | 'Ê' | 'Ë' => 'E',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
        'ñ' => 'n',
        'Ñ' => 'N',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'O',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
        'ý' | 'ÿ' => 'y',
        'Ý' => 'Y',
        _ => c,
    }
}

/// Parse une date `jj.mm.aaaa`, `jj/mm/aaaa` ou `aaaa-mm-jj`.
pub fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    let s = s.trim();
    for fmt in ["%d.%m.%Y", "%d/%m/%Y", "%Y-%m-%d", "%d-%m-%Y", "%m/%d/%Y"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(s, fmt) {
            return Some(d);
        }
    }
    None
}

/// Compare deux valeurs selon le descripteur ; `None` = attribut absent.
pub fn compare_values(desc: &AttributeDesc, a: Option<&Value>, b: Option<&Value>) -> Ordering {
    match desc.compare {
        CompareKind::Ranking => {
            let va = a.map(|v| v.as_int()).filter(|v| *v != 0).unwrap_or(i32::MAX);
            let vb = b.map(|v| v.as_int()).filter(|v| *v != 0).unwrap_or(i32::MAX);
            va.cmp(&vb)
        }
        CompareKind::Date => {
            let da = a.and_then(|v| v.as_str()).and_then(parse_date);
            let db = b.and_then(|v| v.as_str()).and_then(parse_date);
            match (da, db) {
                (Some(x), Some(y)) => x.cmp(&y),
                _ => default_compare(a, b),
            }
        }
        CompareKind::Default => default_compare(a, b),
    }
}

fn default_compare(a: Option<&Value>, b: Option<&Value>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.compare(y),
    }
}

/// Registre des descripteurs (ordre de déclaration conservé : il fixe l'ordre
/// des attributs XML et des colonnes).
pub struct Registry {
    descs: Vec<AttributeDesc>,
}

impl Registry {
    pub fn all(&self) -> &[AttributeDesc] {
        &self.descs
    }

    pub fn by_code(&self, code_name: &str) -> Option<&AttributeDesc> {
        self.descs.iter().find(|d| d.code_name == code_name)
    }

    /// Plusieurs descripteurs partagent le même nom XML (`Statut`) : on
    /// privilégie la portée demandée.
    pub fn by_xml(&self, xml_name: &str, scope: Scope) -> Option<&AttributeDesc> {
        self.descs
            .iter()
            .find(|d| d.xml_name == xml_name && d.scope == scope)
            .or_else(|| self.descs.iter().find(|d| d.xml_name == xml_name))
    }

    /// `GuessDescFromUserName` : retrouve un attribut à partir d'un en-tête
    /// de colonne (import CSV).
    pub fn guess_from_user_name(&self, name: &str) -> Option<&AttributeDesc> {
        let key = collate_key(name.trim());
        self.descs.iter().find(|d| {
            collate_key(d.user_name) == key
                || collate_key(&crate::i18n::tr(d.user_name)) == key
                || collate_key(d.xml_name) == key
                || collate_key(d.code_name) == key
        })
    }

    pub fn swappable(&self) -> impl Iterator<Item = &AttributeDesc> {
        self.descs.iter().filter(|d| d.swappable)
    }

    /// Descripteurs persistants globaux, dans l'ordre d'enregistrement XML.
    pub fn persistent_globals(&self) -> impl Iterator<Item = &AttributeDesc> {
        self.descs.iter().filter(|d| d.scope == Scope::Global && d.persistency == Persistency::Persistent)
    }
}

pub fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(build_registry)
}

pub fn desc(code_name: &str) -> &'static AttributeDesc {
    registry()
        .by_code(code_name)
        .unwrap_or_else(|| panic!("attribut inconnu : {code_name}"))
}

fn dv(list: &[(&str, &str, Option<&str>)]) -> Vec<DiscreteValue> {
    list.iter()
        .enumerate()
        .map(|(i, (xml, user, icon))| DiscreteValue {
            code: i as u32,
            xml: xml.to_string(),
            user: user.to_string(),
            icon: icon.map(|s| s.to_string()),
        })
        .collect()
}

/// Port fidèle de `Application::Prepare` (bloc « Attributes definition »).
fn build_registry() -> Registry {
    use AttrType::*;
    let mut v = Vec::new();
    let mut d;

    d = AttributeDesc::new(UInt, "ref", "ID", "ref");
    d.rights = Rights::Private;
    v.push(d);

    d = AttributeDesc::new(UInt, "plugin_ID", "PluginID", "PluginID");
    d.rights = Rights::Private;
    v.push(d);

    v.push(AttributeDesc::new(UInt, "final_rank", "Classement", "place"));

    d = AttributeDesc::new(Str, "team", "Equipe", "team");
    d.uniqueness = Uniqueness::NotSingular;
    d.has_dynamic_discrete = true;
    d.sorting_enabled = true;
    v.push(d);

    d = AttributeDesc::new(Str, "name", "Nom", "last name");
    d.short_length = 15;
    v.push(d);

    v.push(AttributeDesc::new(Str, "first_name", "Prenom", "first name"));

    d = AttributeDesc::new(Str, "birth_date", "DateNaissance", "date of birth");
    d.compare = CompareKind::Date;
    v.push(d);

    d = AttributeDesc::new(Str, "gender", "Sexe", "gender");
    d.uniqueness = Uniqueness::NotSingular;
    d.free_value_allowed = false;
    d.favorite_look = Look::Graphical;
    d.discrete = dv(&[
        ("M", "Male", Some("glade/images/male.png")),
        ("F", "Female", Some("glade/images/female.png")),
        // Ajout du portage : épreuves mixtes.
        ("X", "Mixed", None),
    ]);
    v.push(d);

    d = AttributeDesc::new(Str, "weapon", "Arme", "weapon");
    d.uniqueness = Uniqueness::NotSingular;
    d.rights = Rights::Private;
    d.free_value_allowed = false;
    d.favorite_look = Look::ShortText;
    d.discrete = dv(&[("X", "Educ", None), ("F", "Foil", None), ("E", "Epée", None), ("S", "Sabre", None)]);
    v.push(d);

    d = AttributeDesc::new(Str, "country", "Nation", "country");
    d.uniqueness = Uniqueness::NotSingular;
    d.favorite_look = Look::ShortText;
    d.discrete_source = Some(DiscreteSource::Selector("countries"));
    d.swappable = true;
    v.push(d);

    d = AttributeDesc::new(Str, "region", "Region", "region");
    d.favorite_look = Look::Graphical;
    d.uniqueness = Uniqueness::NotSingular;
    d.discrete_source = Some(DiscreteSource::Localized("regions"));
    d.swappable = true;
    v.push(d);

    d = AttributeDesc::new(Str, "league", "Ligue", "league");
    d.favorite_look = Look::Graphical;
    d.uniqueness = Uniqueness::NotSingular;
    d.discrete_source = Some(DiscreteSource::Localized("ligues"));
    d.swappable = true;
    v.push(d);

    d = AttributeDesc::new(Str, "club", "Club", "club");
    d.uniqueness = Uniqueness::NotSingular;
    d.discrete_source = Some(DiscreteSource::Localized("clubs"));
    d.swappable = true;
    v.push(d);

    v.push(AttributeDesc::new(Str, "licence", "Licence", "licence"));

    d = AttributeDesc::new(UInt, "workload_rate", "Activite", "rate");
    d.persistency = Persistency::NotPersistent;
    d.favorite_look = Look::Graphical;
    d.rights = Rights::Private;
    v.push(d);

    d = AttributeDesc::new(Str, "category", "Categorie", "level");
    d.uniqueness = Uniqueness::NotSingular;
    v.push(d);

    d = AttributeDesc::new(UInt, "ranking", "Ranking", "ranking");
    d.compare = CompareKind::Ranking;
    v.push(d);

    v.push(AttributeDesc::new(Str, "rating", "Rating", "rating"));

    d = AttributeDesc::new(Bool, "attending", "Presence", "presence");
    d.uniqueness = Uniqueness::NotSingular;
    d.favorite_look = Look::Graphical;
    d.persistency = Persistency::NotPersistent;
    v.push(d);

    d = AttributeDesc::new(Bool, "exported", "Exporte", "exported");
    d.favorite_look = Look::Graphical;
    v.push(d);

    d = AttributeDesc::new(Str, "incident", "Incident", "incident");
    d.scope = Scope::Global;
    d.favorite_look = Look::Graphical;
    d.rights = Rights::Private;
    d.discrete = dv(&[
        ("A", "Absent", Some("glade/images/minus.png")),
        ("R", "Latecomer", Some("glade/images/plus.png")),
    ]);
    v.push(d);

    let status_values = dv(&[
        ("Q", "Qualified", Some("glade/images/normal.png")),
        ("N", "Not qualified", Some("glade/images/exit.png")),
        ("A", "Withdrawal", Some("glade/images/ambulance.png")),
        ("E", "Excluded", Some("glade/images/black_card.png")),
        ("F", "Forfeit", Some("glade/images/normal.png")),
    ]);

    d = AttributeDesc::new(Str, "global_status", "Statut", "global status");
    d.scope = Scope::Global;
    d.rights = Rights::Private;
    d.favorite_look = Look::Graphical;
    d.discrete = status_values.clone();
    v.push(d);

    d = AttributeDesc::new(Str, "status", "Statut", "status");
    d.scope = Scope::Local;
    d.favorite_look = Look::Graphical;
    d.discrete = status_values;
    v.push(d);

    for (code, xml, user) in [
        ("custom1", "Divers1", "♥"),
        ("custom2", "Divers2", "♣"),
        ("custom3", "Divers3", "♠"),
        ("custom4", "Divers4", "♦"),
    ] {
        d = AttributeDesc::new(Str, code, xml, user);
        d.uniqueness = Uniqueness::NotSingular;
        v.push(d);
    }

    // Entraînement des jeunes (extension du portage) : groupe du tireur.
    d = AttributeDesc::new(UInt, "youth_group", "Groupe", "Group");
    v.push(d);

    d = AttributeDesc::new(Str, "cyphered_password", "MotDePasseChiffre", "cyphered password");
    d.rights = Rights::Private;
    v.push(d);

    // Données non persistantes
    d = AttributeDesc::new(Str, "IP", "IP", "IP address");
    d.persistency = Persistency::NotPersistent;
    d.rights = Rights::Private;
    v.push(d);

    // Clé personnelle d'un arbitre (QR code de son téléphone, PIN) :
    // enregistrée, pour que les QR codes imprimés restent valables.
    d = AttributeDesc::new(Str, "password", "MotDePasse", "Password");
    d.rights = Rights::Private;
    v.push(d);

    for (ty, code, xml, user) in [
        (UInt, "pool_nr", "PoolID", "pool #"),
        (UInt, "victories_count", "Victoires", "Victories"),
        (UInt, "bouts_count", "Assauts", "Bouts"),
        (UInt, "victories_ratio", "RatioVictoires", "Vict./Bouts (‰)"),
        (Int, "indice", "Indice", "index"),
        (UInt, "HS", "TD", "Hits scored"),
        (Str, "strip", "Piste", "strip"),
        (Str, "time", "Heure", "time"),
    ] {
        d = AttributeDesc::new(ty, code, xml, user);
        d.persistency = Persistency::NotPersistent;
        d.scope = Scope::Local;
        v.push(d);
    }

    for (code, xml, user) in [("rank", "rank", "place"), ("stage_start_rank", "RangDeDepart", "Round start rank")] {
        d = AttributeDesc::new(UInt, code, xml, user);
        d.persistency = Persistency::NotPersistent;
        d.rights = Rights::Private;
        d.scope = Scope::Local;
        v.push(d);
    }

    d = AttributeDesc::new(Bool, "promoted", "Promu", "promoted");
    d.persistency = Persistency::NotPersistent;
    d.scope = Scope::Local;
    d.favorite_look = Look::Graphical;
    v.push(d);

    d = AttributeDesc::new(Str, "connection", "Connexion", "connection");
    d.persistency = Persistency::NotPersistent;
    d.rights = Rights::Private;
    d.scope = Scope::Global;
    d.favorite_look = Look::Graphical;
    d.discrete = dv(&[
        ("Broken", "Broken", Some("glade/images/red.png")),
        ("Waiting", "Waiting", Some("glade/images/orange.png")),
        ("OK", "OK", Some("glade/images/green.png")),
        ("Manual", "Manual", None),
    ]);
    v.push(d);

    Registry { descs: v }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_lookup() {
        assert_eq!(registry().by_code("name").unwrap().xml_name, "Nom");
        assert_eq!(registry().by_xml("Statut", Scope::Local).unwrap().code_name, "status");
        assert_eq!(registry().by_xml("Statut", Scope::Global).unwrap().code_name, "global_status");
        assert_eq!(registry().swappable().count(), 4);
    }

    #[test]
    fn ranking_compare_puts_zero_last() {
        let d = desc("ranking");
        assert_eq!(compare_values(d, Some(&Value::UInt(0)), Some(&Value::UInt(5))), Ordering::Greater);
        assert_eq!(compare_values(d, Some(&Value::UInt(3)), Some(&Value::UInt(5))), Ordering::Less);
    }

    #[test]
    fn leading_int() {
        assert_eq!(parse_leading_int("12abc"), 12);
        assert_eq!(parse_leading_int("-4"), -4);
        assert_eq!(parse_leading_int("x"), 0);
    }
}
