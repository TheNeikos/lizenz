// Marcel Müller © 2025
//
// Licensed under the EUPL

use std::collections::HashMap;
use std::io::Write;

use camino::Utf8Path;
use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use glob_match::glob_match;
use miette::Context;
use miette::IntoDiagnostic;
use miette::bail;
use miette::miette;
use serde::Deserialize;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::warn;
use tracing_subscriber::EnvFilter;
use tree_sitter_language::LanguageFn;

#[derive(Debug, Parser)]
pub struct Args {
    /// A directory containing tree sitter grammar shared objects
    #[clap(short, long, env)]
    pub tree_sitter_grammars: Utf8PathBuf,

    #[clap(short, long)]
    pub config_path: Option<Utf8PathBuf>,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Verify {
        /// List of files to check their licence on
        files: Vec<Utf8PathBuf>,
    },
    Fix {
        /// List of files to check their licences and try to fix them
        files: Vec<Utf8PathBuf>,
    },
}

#[derive(Debug, Default, Deserialize)]
pub struct LicenseConfig {
    text: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    license: LicenseConfig,
    #[serde(default)]
    languages: HashMap<String, LanguageConfig>,
}

#[derive(Debug, Deserialize)]
pub struct CommentConfig {
    tree_sitter_name: String,
    comment_kind: CommentKind,
    preferred: bool,
}

#[derive(Debug, Deserialize)]
pub enum CommentKind {
    Single(String),
    Multi {
        start: String,
        end: String,
        between: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
pub struct LanguageConfig {
    file_endings: Vec<String>,
    comments: Vec<CommentConfig>,
}

fn default_languages() -> HashMap<String, LanguageConfig> {
    [
        (
            String::from("bash"),
            LanguageConfig {
                file_endings: vec![String::from("*.sh")],
                comments: vec![],
            },
        ),
        (
            String::from("rust"),
            LanguageConfig {
                file_endings: vec![String::from("*.rs")],
                comments: vec![
                    CommentConfig {
                        tree_sitter_name: String::from("block_comment"),
                        comment_kind: CommentKind::Multi {
                            start: String::from("/*"),
                            between: Some(String::from("*")),
                            end: String::from("*/"),
                        },
                        preferred: false,
                    },
                    CommentConfig {
                        tree_sitter_name: String::from("line_comment"),
                        comment_kind: CommentKind::Single(String::from("//")),
                        preferred: true,
                    },
                ],
            },
        ),
        (
            String::from("toml"),
            LanguageConfig {
                file_endings: vec![String::from("*.toml")],
                comments: vec![CommentConfig {
                    tree_sitter_name: String::from("comment"),
                    comment_kind: CommentKind::Single(String::from("#")),
                    preferred: true,
                }],
            },
        ),
    ]
    .into()
}

struct Language {
    _name: String,
    _library: libloading::Library,
    language_fn: LanguageFn,
}

fn main() -> miette::Result<()> {
    tracing_subscriber::fmt::fmt()
        .pretty()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let langs = load_languages(&args)?;

    let mut config: Config = if let Some(config_path) = args
        .config_path
        .as_ref()
        .map(|path| path.as_path())
        .or_else(|| Some(Utf8Path::new("./lizenz.toml")))
    {
        match load_configuration(config_path).with_context(|| {
            miette!(
                "While loading config at {}, current working directory is {}",
                config_path
                    .canonicalize_utf8()
                    .unwrap_or(config_path.to_path_buf()),
                std::env::current_dir().unwrap_or_default().display()
            )
        }) {
            Ok(conf) => conf,
            Err(error) => {
                return Err(error
                    .context("Could not load configuration, please verify errors and try again"));
            }
        }
    } else {
        bail!("Could not find configuration, nothing to be done");
    };

    for (name, lang) in default_languages() {
        config.languages.entry(name).or_insert(lang);
    }

    match args.command {
        Command::Verify { files } => {
            for file in files {
                debug!("Checking {}", file);
                verify_file(&langs, &config, &file)?;
            }
        }
        Command::Fix { files } => {
            for file in files {
                debug!("Checking {}", file);
                let is_valid = verify_file(&langs, &config, &file)?;

                if !is_valid {
                    let (language_config, parser) = load_language(&langs, &config, &file)?;

                    let Some(conf) = language_config
                        .comments
                        .iter()
                        .find(|conf| conf.preferred)
                        .or_else(|| language_config.comments.first())
                    else {
                        bail!(
                            "No comment configuration exists for language {}",
                            parser.language().unwrap().name().unwrap_or_default(),
                        );
                    };

                    let header = match &conf.comment_kind {
                        CommentKind::Single(prefix) => config
                            .license
                            .text
                            .lines()
                            .map(|line| {
                                if line.is_empty() {
                                    format!("{prefix}\n")
                                } else {
                                    format!("{prefix} {line}\n")
                                }
                            })
                            .collect::<String>(),
                        CommentKind::Multi {
                            start,
                            end,
                            between,
                        } => {
                            let line_count = config.license.text.lines().count();

                            match line_count {
                                0..=1 => {
                                    format!("{start} {} {end}", config.license.text)
                                }
                                2.. => {
                                    let mut lines = config.license.text.lines();
                                    let mut header = format!(
                                        "{start} {}",
                                        lines.next().expect("We know length is at least 2")
                                    );

                                    header.extend(lines.by_ref().take(line_count - 2).map(
                                        |line| {
                                            format!(
                                                "{} {}",
                                                between.as_deref().unwrap_or_default(),
                                                line
                                            )
                                        },
                                    ));

                                    header.push_str(&format!(
                                        " {} {end}",
                                        lines.next().expect("We know length is at least 2")
                                    ));

                                    header
                                }
                            }
                        }
                    };

                    let old_content = std::fs::read(&file)
                        .into_diagnostic()
                        .with_context(|| miette!("While reading the file {file}"))?;

                    let mut file_handle = std::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .open(&file)
                        .into_diagnostic()
                        .with_context(|| miette!("Could not open file to write to it at {file}"))?;

                    file_handle
                        .write_all(header.as_bytes())
                        .into_diagnostic()
                        .with_context(|| miette!("Could not write new header at {file}"))?;

                    file_handle
                        .write_all(&old_content)
                        .into_diagnostic()
                        .with_context(|| miette!("Could not write new header at {file}"))?;
                }
            }
        }
    }

    Ok(())
}

fn verify_file(
    langs: &HashMap<String, Language>,
    config: &Config,
    file: &Utf8Path,
) -> Result<bool, miette::Error> {
    let (language_config, mut parser) = load_language(langs, config, file)?;
    let text = std::fs::read_to_string(file).into_diagnostic()?;
    let Some(tree) = parser.parse(&text, None) else {
        miette::bail!("Could not parse file")
    };
    let mut cursor = tree.walk();
    let mut comments = String::new();
    for child in tree.root_node().named_children(&mut cursor) {
        if let Some(conf) = language_config
            .comments
            .iter()
            .find(|n| n.tree_sitter_name == child.grammar_name())
        {
            let text = child.utf8_text(text.as_bytes()).into_diagnostic()?;

            match &conf.comment_kind {
                CommentKind::Single(prefix) => {
                    comments.push_str(text.trim_start_matches(prefix).trim());
                    comments.push('\n');
                }
                CommentKind::Multi {
                    start,
                    end,
                    between,
                } => {
                    comments.push_str(
                        &text
                            .trim_start_matches(start)
                            .trim_end_matches(end)
                            .lines()
                            .map(|line| {
                                line.trim_start_matches(between.as_deref().unwrap_or_default())
                                    .trim()
                            })
                            .collect::<Vec<&str>>()
                            .join("\n"),
                    );
                }
            }
        }
    }

    let comments = comments
        .lines()
        .take(config.license.text.lines().count())
        .collect::<Vec<&str>>()
        .join("\n");

    if comments.trim() == config.license.text.trim() {
        Ok(true)
    } else {
        debug!("Expected: {}\nGot: {comments}", config.license.text);
        Ok(false)
    }
}

fn load_language<'a>(
    langs: &HashMap<String, Language>,
    config: &'a Config,
    file: &Utf8Path,
) -> Result<(&'a LanguageConfig, tree_sitter::Parser), miette::Error> {
    let language = config.languages.iter().find(|(_name, globs)| {
        globs
            .file_endings
            .iter()
            .any(|glob| glob_match(glob, file.file_name().unwrap()))
    });
    let Some((name, language_config)) = language else {
        bail!("Could not determine language for {}", file);
    };
    let Some(language) = langs.get(name) else {
        bail!(
            "Found language {} but no tree-sitter grammar exists for it",
            name
        );
    };
    let grammar = tree_sitter::Language::new(language.language_fn);
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&grammar).into_diagnostic()?;
    Ok((language_config, parser))
}

fn load_configuration(config_path: &Utf8Path) -> Result<Config, miette::Error> {
    toml::from_str(&std::fs::read_to_string(config_path).into_diagnostic()?).into_diagnostic()
}

fn load_languages(args: &Args) -> Result<HashMap<String, Language>, miette::Error> {
    let mut langs = HashMap::new();
    for file in args
        .tree_sitter_grammars
        .read_dir_utf8()
        .into_diagnostic()?
    {
        let entry = match file {
            Ok(entry) => entry,
            Err(error) => {
                error!(?error, "Could not read directory entry");
                continue;
            }
        };

        match entry.file_type() {
            Ok(filetype) => {
                if filetype.is_dir() {
                    debug!("Skipping {}, as it is a directory", entry.path());
                    continue;
                }
            }
            Err(error) => {
                error!(?error, "Could not get entry type at {}", entry.path());
                continue;
            }
        }

        let Some(lang_name) = entry.path().file_stem() else {
            warn!("Found {}, but could not determine its name", entry.path());
            continue;
        };
        let language = load_ts_lib(entry.path(), lang_name)
            .with_context(|| format!("While trying to load {}", entry.path()))?;

        langs.insert(lang_name.to_string(), language);
    }
    Ok(langs)
}

fn load_ts_lib(entry: &camino::Utf8Path, lang_name: &str) -> Result<Language, miette::Error> {
    let symbol = format!("tree_sitter_{lang_name}");
    let library;
    let language_fn;

    unsafe {
        library = libloading::Library::new(entry).into_diagnostic()?;
        let lang_constructor: libloading::Symbol<unsafe extern "C" fn() -> *const ()> =
            library.get(symbol.as_bytes()).into_diagnostic()?;
        language_fn = LanguageFn::from_raw(*lang_constructor);
    }
    Ok(Language {
        _name: lang_name.to_string(),
        _library: library,
        language_fn,
    })
}
