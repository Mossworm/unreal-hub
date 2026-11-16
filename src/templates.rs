use crate::unreal::{ini_values, is_uproject};
use anyhow::{Context as _, Result};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::str::Chars;

const TEMPLATE_DEFS_SECTION: &str = "/Script/GameProjectGeneration.TemplateProjectDefs";
const TEMPLATE_CATEGORIES_SECTION: &str = "/Script/GameProjectGeneration.TemplateCategories";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UeValue {
    Text(String),
    Group(Vec<UeEntry>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UeEntry {
    pub key: Option<String>,
    pub value: UeValue,
}

impl UeValue {
    pub fn text(&self) -> Option<&str> {
        match self {
            UeValue::Text(text) => Some(text),
            UeValue::Group(_) => None,
        }
    }

    pub fn field(&self, key: &str) -> Option<&UeValue> {
        match self {
            UeValue::Group(entries) => entries
                .iter()
                .find(|entry| {
                    entry
                        .key
                        .as_deref()
                        .is_some_and(|entry_key| entry_key.eq_ignore_ascii_case(key))
                })
                .map(|entry| &entry.value),
            UeValue::Text(_) => None,
        }
    }

    pub fn field_text(&self, key: &str) -> Option<&str> {
        self.field(key).and_then(UeValue::text)
    }

    pub fn items(&self) -> Vec<&UeValue> {
        match self {
            UeValue::Group(entries) => entries.iter().map(|entry| &entry.value).collect(),
            UeValue::Text(text) if text.is_empty() => Vec::new(),
            UeValue::Text(_) => vec![self],
        }
    }
}

struct UeParser<'a> {
    characters: Peekable<Chars<'a>>,
}

impl UeParser<'_> {
    fn skip_whitespace(&mut self) {
        while self
            .characters
            .peek()
            .is_some_and(|character| character.is_whitespace())
        {
            self.characters.next();
        }
    }

    fn value(&mut self) -> UeValue {
        self.skip_whitespace();
        match self.characters.peek() {
            Some('(') => {
                self.characters.next();
                self.group()
            }
            Some('"') => {
                self.characters.next();
                UeValue::Text(self.quoted())
            }
            _ => UeValue::Text(self.bare(false)),
        }
    }

    fn group(&mut self) -> UeValue {
        let mut entries = Vec::new();
        loop {
            self.skip_whitespace();
            match self.characters.peek() {
                None => break,
                Some(')') => {
                    self.characters.next();
                    break;
                }
                Some(',') => {
                    self.characters.next();
                    continue;
                }
                _ => {}
            }
            entries.push(self.entry());
        }
        UeValue::Group(entries)
    }

    fn entry(&mut self) -> UeEntry {
        self.skip_whitespace();
        if matches!(self.characters.peek(), Some('(') | Some('"')) {
            return UeEntry {
                key: None,
                value: self.value(),
            };
        }
        let token = self.bare(true);
        if self.characters.peek() == Some(&'=') {
            self.characters.next();
            UeEntry {
                key: Some(token),
                value: self.value(),
            }
        } else {
            UeEntry {
                key: None,
                value: UeValue::Text(token),
            }
        }
    }

    fn quoted(&mut self) -> String {
        let mut text = String::new();
        while let Some(character) = self.characters.next() {
            match character {
                '"' => break,
                '\\' => match self.characters.next() {
                    Some('n') => text.push('\n'),
                    Some(escaped) => text.push(escaped),
                    None => break,
                },
                other => text.push(other),
            }
        }
        text
    }

    fn bare(&mut self, stop_at_equals: bool) -> String {
        let mut text = String::new();
        while let Some(&character) = self.characters.peek() {
            if character == ',' || character == ')' || (stop_at_equals && character == '=') {
                break;
            }
            text.push(character);
            self.characters.next();
        }
        text.trim().to_string()
    }
}

pub fn parse_ue_value(input: &str) -> UeValue {
    UeParser {
        characters: input.chars().peekable(),
    }
    .value()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalizedText {
    pub language: String,
    pub text: String,
}

fn parse_localized(values: &[String]) -> Vec<LocalizedText> {
    values
        .iter()
        .map(|value| parse_ue_value(value))
        .flat_map(|value| localized_entries(&value))
        .collect()
}

fn localized_entries(value: &UeValue) -> Vec<LocalizedText> {
    if let (Some(language), Some(text)) = (value.field_text("Language"), value.field_text("Text")) {
        return vec![LocalizedText {
            language: language.to_string(),
            text: text.to_string(),
        }];
    }
    value
        .items()
        .into_iter()
        .filter(|item| matches!(item, UeValue::Group(_)))
        .flat_map(localized_entries)
        .collect()
}

pub fn choose_localized(texts: &[LocalizedText], language: &str) -> String {
    texts
        .iter()
        .find(|text| text.language.eq_ignore_ascii_case(language))
        .or_else(|| {
            texts
                .iter()
                .find(|text| text.language.eq_ignore_ascii_case("en"))
        })
        .or_else(|| texts.first())
        .map(|text| text.text.replace('\n', " "))
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderRename {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    pub extensions: Vec<String>,
    pub from: String,
    pub to: String,
    pub case_sensitive: bool,
}

impl Replacement {
    pub fn applies_to(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedContentPack {
    pub mount_name: String,
    pub detail_levels: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemplateSetting {
    HardwareTarget,
    GraphicsPreset,
}

impl TemplateSetting {
    fn ini_name(self) -> &'static str {
        match self {
            TemplateSetting::HardwareTarget => "HardwareTarget",
            TemplateSetting::GraphicsPreset => "GraphicsPreset",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateDefs {
    pub display_names: Vec<LocalizedText>,
    pub descriptions: Vec<LocalizedText>,
    pub categories: Vec<String>,
    pub sort_key: String,
    pub is_blank: bool,
    pub allow_project_creation: bool,
    pub hidden_settings: Vec<String>,
    pub folders_to_ignore: Vec<String>,
    pub files_to_ignore: Vec<String>,
    pub folder_renames: Vec<FolderRename>,
    pub filename_replacements: Vec<Replacement>,
    pub replacements_in_files: Vec<Replacement>,
    pub shared_content_packs: Vec<SharedContentPack>,
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes"
    )
}

fn text_values(values: Vec<String>) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| parse_ue_value(value).text().map(str::to_string))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_replacements(values: Vec<String>) -> Vec<Replacement> {
    values
        .iter()
        .map(|value| parse_ue_value(value))
        .filter_map(|value| {
            Some(Replacement {
                extensions: value
                    .field("Extensions")
                    .map(|extensions| {
                        extensions
                            .items()
                            .into_iter()
                            .filter_map(UeValue::text)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                from: value.field_text("From")?.to_string(),
                to: value.field_text("To")?.to_string(),
                case_sensitive: value.field_text("bCaseSensitive").is_some_and(parse_bool),
            })
        })
        .collect()
}

pub fn parse_template_defs(text: &str) -> TemplateDefs {
    let values = |key: &str| ini_values(text, TEMPLATE_DEFS_SECTION, key);
    let single = |key: &str| values(key).into_iter().last();
    TemplateDefs {
        display_names: parse_localized(&values("LocalizedDisplayNames")),
        descriptions: parse_localized(&values("LocalizedDescriptions")),
        categories: text_values(values("Categories")),
        sort_key: single("SortKey")
            .and_then(|value| parse_ue_value(&value).text().map(str::to_string))
            .unwrap_or_default(),
        is_blank: single("bIsBlank").is_some_and(|value| parse_bool(&value)),
        allow_project_creation: single("bAllowProjectCreation")
            .is_none_or(|value| parse_bool(&value)),
        hidden_settings: text_values(values("HiddenSettings")),
        folders_to_ignore: text_values(values("FoldersToIgnore")),
        files_to_ignore: text_values(values("FilesToIgnore")),
        folder_renames: values("FolderRenames")
            .iter()
            .map(|value| parse_ue_value(value))
            .filter_map(|value| {
                Some(FolderRename {
                    from: value.field_text("From")?.to_string(),
                    to: value.field_text("To")?.to_string(),
                })
            })
            .collect(),
        filename_replacements: parse_replacements(values("FilenameReplacements")),
        replacements_in_files: parse_replacements(values("ReplacementsInFiles")),
        shared_content_packs: values("SharedContentPacks")
            .iter()
            .map(|value| parse_ue_value(value))
            .filter_map(|value| {
                Some(SharedContentPack {
                    mount_name: value.field_text("MountName")?.to_string(),
                    detail_levels: value
                        .field("DetailLevels")
                        .map(|levels| {
                            levels
                                .items()
                                .into_iter()
                                .filter_map(UeValue::text)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect(),
    }
}

pub fn fix_template_string(text: &str, template_name: &str, project_name: &str) -> String {
    text.replace("%TEMPLATENAME%", template_name)
        .replace("%TEMPLATENAME_UPPERCASE%", &template_name.to_uppercase())
        .replace("%TEMPLATENAME_LOWERCASE%", &template_name.to_lowercase())
        .replace("%PROJECTNAME%", project_name)
        .replace("%PROJECTNAME_UPPERCASE%", &project_name.to_uppercase())
        .replace("%PROJECTNAME_LOWERCASE%", &project_name.to_lowercase())
}

impl TemplateDefs {
    pub fn hides(&self, setting: TemplateSetting) -> bool {
        self.hidden_settings.iter().any(|hidden| {
            hidden.eq_ignore_ascii_case("All") || hidden.eq_ignore_ascii_case(setting.ini_name())
        })
    }

    pub fn display_name(&self) -> String {
        choose_localized(&self.display_names, crate::i18n::current().code())
    }

    pub fn description(&self) -> String {
        choose_localized(&self.descriptions, crate::i18n::current().code())
    }

    pub fn fixed_up(&self, template_name: &str, project_name: &str) -> TemplateDefs {
        let fix = |text: &String| fix_template_string(text, template_name, project_name);
        let fix_replacement = |replacement: &Replacement| Replacement {
            from: fix(&replacement.from),
            to: fix(&replacement.to),
            ..replacement.clone()
        };
        TemplateDefs {
            folders_to_ignore: self.folders_to_ignore.iter().map(fix).collect(),
            files_to_ignore: self.files_to_ignore.iter().map(fix).collect(),
            folder_renames: self
                .folder_renames
                .iter()
                .map(|rename| FolderRename {
                    from: fix(&rename.from),
                    to: fix(&rename.to),
                })
                .collect(),
            filename_replacements: self
                .filename_replacements
                .iter()
                .map(fix_replacement)
                .collect(),
            replacements_in_files: self
                .replacements_in_files
                .iter()
                .map(fix_replacement)
                .collect(),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub folder: PathBuf,
    pub uproject: PathBuf,
    pub name: String,
    pub defs: TemplateDefs,
    pub generates_code: bool,
    pub thumbnail: Option<PathBuf>,
    pub preview: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateGroup {
    pub key: String,
    pub display_name: String,
    pub description: String,
    pub categories: Vec<String>,
    pub sort_key: String,
    pub blueprint: Option<Template>,
    pub cpp: Option<Template>,
}

impl TemplateGroup {
    pub fn representative(&self) -> Option<&Template> {
        self.blueprint.as_ref().or(self.cpp.as_ref())
    }

    pub fn thumbnail(&self) -> Option<&PathBuf> {
        self.representative()
            .and_then(|template| template.thumbnail.as_ref())
    }

    pub fn preview(&self) -> Option<&PathBuf> {
        self.representative()
            .and_then(|template| template.preview.as_ref().or(template.thumbnail.as_ref()))
    }

    pub fn is_blank(&self) -> bool {
        self.representative()
            .is_some_and(|template| template.defs.is_blank)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateCategory {
    pub key: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateCatalog {
    pub categories: Vec<TemplateCategory>,
    pub groups: Vec<TemplateGroup>,
}

impl TemplateCatalog {
    pub fn groups_in(&self, category: &str) -> Vec<&TemplateGroup> {
        self.groups
            .iter()
            .filter(|group| {
                group
                    .categories
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(category))
            })
            .collect()
    }

    pub fn group(&self, key: &str) -> Option<&TemplateGroup> {
        self.groups.iter().find(|group| group.key == key)
    }

    pub fn default_group(&self) -> Option<&TemplateGroup> {
        self.groups
            .iter()
            .find(|group| {
                group.is_blank()
                    && group
                        .categories
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case("Games"))
            })
            .or_else(|| self.groups.iter().find(|group| group.is_blank()))
            .or_else(|| self.groups.first())
    }
}

pub fn parse_template_categories(text: &str, language: &str) -> Vec<TemplateCategory> {
    ini_values(text, TEMPLATE_CATEGORIES_SECTION, "Categories")
        .iter()
        .map(|value| parse_ue_value(value))
        .filter_map(|value| {
            let key = value.field_text("Key")?.to_string();
            let display_names = value
                .field("LocalizedDisplayNames")
                .map(localized_entries)
                .unwrap_or_default();
            let display_name = choose_localized(&display_names, language);
            Some(TemplateCategory {
                display_name: if display_name.is_empty() {
                    key.clone()
                } else {
                    display_name
                },
                key,
            })
        })
        .collect()
}

pub fn group_key(template_name: &str) -> String {
    template_name
        .strip_suffix("_BP")
        .or_else(|| template_name.strip_suffix("BP"))
        .unwrap_or(template_name)
        .to_string()
}

fn find_media(folder: &Path, stem: &str) -> Option<PathBuf> {
    let candidate = folder.join("Media").join(format!("{stem}.png"));
    candidate.is_file().then_some(candidate)
}

fn load_template(folder: &Path) -> Option<Template> {
    let defs_path = folder.join("Config").join("TemplateDefs.ini");
    let defs_text = std::fs::read_to_string(&defs_path).ok()?;
    let defs = parse_template_defs(&defs_text);
    if !defs.allow_project_creation {
        return None;
    }
    let mut uprojects: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_uproject(path) && path.is_file())
        .collect();
    uprojects.sort();
    let folder_name = folder.file_name()?.to_string_lossy().to_string();
    let uproject = uprojects
        .iter()
        .find(|path| {
            path.file_stem()
                .is_some_and(|stem| stem.to_string_lossy() == folder_name)
        })
        .or_else(|| uprojects.first())?
        .clone();
    let name = uproject.file_stem()?.to_string_lossy().to_string();
    let thumbnail = find_media(folder, &name);
    let preview = find_media(folder, &format!("{name}_Preview"));
    Some(Template {
        folder: folder.to_path_buf(),
        generates_code: folder.join("Source").is_dir(),
        uproject,
        name,
        defs,
        thumbnail,
        preview,
    })
}

pub fn build_catalog(
    templates: Vec<Template>,
    mut categories: Vec<TemplateCategory>,
) -> TemplateCatalog {
    let mut groups: Vec<TemplateGroup> = Vec::new();
    for template in templates {
        let key = group_key(&template.name);
        let position = match groups.iter().position(|group| group.key == key) {
            Some(position) => position,
            None => {
                groups.push(TemplateGroup {
                    key: key.clone(),
                    display_name: template.defs.display_name(),
                    description: template.defs.description(),
                    categories: template.defs.categories.clone(),
                    sort_key: template.defs.sort_key.clone(),
                    blueprint: None,
                    cpp: None,
                });
                groups.len() - 1
            }
        };
        let Some(group) = groups.get_mut(position) else {
            continue;
        };
        if group.display_name.is_empty() {
            group.display_name = template.defs.display_name();
        }
        if template.generates_code {
            group.cpp = Some(template);
        } else {
            group.blueprint = Some(template);
        }
    }
    for group in &mut groups {
        if group.display_name.is_empty() {
            group.display_name = group.key.clone();
        }
    }
    groups.sort_by(|left, right| {
        left.sort_key
            .cmp(&right.sort_key)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    for group in &groups {
        for category in &group.categories {
            if !categories
                .iter()
                .any(|known| known.key.eq_ignore_ascii_case(category))
            {
                categories.push(TemplateCategory {
                    key: category.clone(),
                    display_name: category.clone(),
                });
            }
        }
    }
    let catalog = TemplateCatalog { categories, groups };
    let used_categories = catalog
        .categories
        .iter()
        .filter(|category| !catalog.groups_in(&category.key).is_empty())
        .cloned()
        .collect();
    TemplateCatalog {
        categories: used_categories,
        ..catalog
    }
}

pub fn load_catalog(engine_root: &Path) -> Result<TemplateCatalog> {
    let templates_dir = engine_root.join("Templates");
    let entries = std::fs::read_dir(&templates_dir).with_context(|| {
        trf!(
            "템플릿 폴더를 읽을 수 없습니다: {}",
            "Cannot read the templates folder: {}",
            templates_dir.display()
        )
    })?;
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    let templates = folders
        .iter()
        .filter_map(|folder| load_template(folder))
        .collect();
    let categories = std::fs::read_to_string(templates_dir.join("TemplateCategories.ini"))
        .map(|text| parse_template_categories(&text, crate::i18n::current().code()))
        .unwrap_or_default();
    Ok(build_catalog(templates, categories))
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIRD_PERSON_DEFS: &str = include_str!("../tests/fixtures/TemplateDefs_ThirdPerson.ini");
    const BLANK_DEFS: &str = include_str!("../tests/fixtures/TemplateDefs_Blank.ini");
    const CATEGORIES: &str = include_str!("../tests/fixtures/TemplateCategories.ini");

    #[test]
    fn parses_nested_struct_values() {
        let value = parse_ue_value(
            r#"(Extensions=("cpp","h"),From="%TEMPLATENAME%",To="%PROJECTNAME%",bCaseSensitive=true)"#,
        );
        assert_eq!(value.field_text("From"), Some("%TEMPLATENAME%"));
        assert_eq!(value.field_text("bCaseSensitive"), Some("true"));
        let extensions: Vec<_> = value
            .field("Extensions")
            .unwrap()
            .items()
            .into_iter()
            .filter_map(UeValue::text)
            .collect();
        assert_eq!(extensions, vec!["cpp", "h"]);
        let bare = parse_ue_value(r#"(MountName="Input",DetailLevels=(Standard,High))"#);
        let levels: Vec<_> = bare
            .field("DetailLevels")
            .unwrap()
            .items()
            .into_iter()
            .filter_map(UeValue::text)
            .collect();
        assert_eq!(levels, vec!["Standard", "High"]);
    }

    #[test]
    fn parses_template_defs() {
        let defs = parse_template_defs(THIRD_PERSON_DEFS);
        assert_eq!(choose_localized(&defs.display_names, "ko"), "3인칭");
        assert_eq!(defs.categories, vec!["Games"]);
        assert!(!defs.is_blank);
        assert!(defs.allow_project_creation);
        assert!(
            defs.folders_to_ignore
                .contains(&"Content/ThirdPerson/Animations".to_string())
        );
        assert!(
            defs.files_to_ignore
                .contains(&"%TEMPLATENAME%.uproject".to_string())
        );
        assert_eq!(
            defs.folder_renames.first(),
            Some(&FolderRename {
                from: "Source/%TEMPLATENAME%".into(),
                to: "Source/%PROJECTNAME%".into(),
            })
        );
        assert_eq!(defs.filename_replacements.len(), 3);
        assert_eq!(
            defs.replacements_in_files.last(),
            Some(&Replacement {
                extensions: vec!["cpp".into(), "h".into(), "ini".into(), "cs".into()],
                from: "%TEMPLATENAME%".into(),
                to: "%PROJECTNAME%".into(),
                case_sensitive: false,
            })
        );
        assert_eq!(
            defs.shared_content_packs,
            vec![
                SharedContentPack {
                    mount_name: "LevelPrototyping".into(),
                    detail_levels: vec!["High".into()],
                },
                SharedContentPack {
                    mount_name: "Characters".into(),
                    detail_levels: vec!["High".into()],
                },
                SharedContentPack {
                    mount_name: "Input".into(),
                    detail_levels: vec!["High".into()],
                },
            ]
        );
        assert!(!defs.hides(TemplateSetting::HardwareTarget));
    }

    #[test]
    fn parses_blank_template_flags() {
        let defs = parse_template_defs(BLANK_DEFS);
        assert!(defs.is_blank);
        assert_eq!(defs.sort_key, "_1");
        assert!(defs.hidden_settings.is_empty());
        assert!(defs.shared_content_packs.is_empty());
    }

    #[test]
    fn hidden_all_hides_every_setting() {
        let defs = parse_template_defs(
            "[/Script/GameProjectGeneration.TemplateProjectDefs]\nHiddenSettings=All\n",
        );
        assert!(defs.hides(TemplateSetting::HardwareTarget));
        assert!(defs.hides(TemplateSetting::GraphicsPreset));
    }

    #[test]
    fn fixes_up_template_strings() {
        let defs = parse_template_defs(THIRD_PERSON_DEFS).fixed_up("TP_ThirdPerson", "MyGame");
        assert!(
            defs.files_to_ignore
                .contains(&"TP_ThirdPerson.uproject".to_string())
        );
        assert_eq!(
            defs.folder_renames.first(),
            Some(&FolderRename {
                from: "Source/TP_ThirdPerson".into(),
                to: "Source/MyGame".into(),
            })
        );
        assert_eq!(
            defs.replacements_in_files
                .first()
                .map(|replacement| replacement.from.as_str()),
            Some("TP_THIRDPERSON")
        );
    }

    #[test]
    fn parses_categories_with_korean_names() {
        let categories = parse_template_categories(CATEGORIES, "ko");
        assert_eq!(
            categories.first(),
            Some(&TemplateCategory {
                key: "Games".into(),
                display_name: "게임".into(),
            })
        );
        assert_eq!(
            categories
                .get(1)
                .map(|category| category.display_name.as_str()),
            Some("영화, TV 및 라이브 이벤트")
        );
    }

    #[test]
    fn groups_blueprint_and_cpp_templates() {
        assert_eq!(group_key("TP_ThirdPersonBP"), "TP_ThirdPerson");
        assert_eq!(group_key("TP_UEIntro_BP"), "TP_UEIntro");
        assert_eq!(group_key("TP_Blank"), "TP_Blank");

        let template = |name: &str, code: bool, defs: &str| Template {
            folder: PathBuf::from(name),
            uproject: PathBuf::from(format!("{name}/{name}.uproject")),
            name: name.into(),
            defs: parse_template_defs(defs),
            generates_code: code,
            thumbnail: None,
            preview: None,
        };
        let catalog = build_catalog(
            vec![
                template("TP_ThirdPerson", true, THIRD_PERSON_DEFS),
                template("TP_ThirdPersonBP", false, THIRD_PERSON_DEFS),
                template("TP_BlankBP", false, BLANK_DEFS),
            ],
            parse_template_categories(CATEGORIES, "ko"),
        );
        assert_eq!(catalog.groups.len(), 2);
        let blank = catalog.default_group().unwrap();
        assert_eq!(blank.key, "TP_Blank");
        assert!(blank.cpp.is_none());
        let third_person = catalog.group("TP_ThirdPerson").unwrap();
        assert!(third_person.cpp.is_some() && third_person.blueprint.is_some());
        let category_keys: Vec<_> = catalog.categories.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(category_keys, vec!["Games"]);
    }
}
