use std::collections::HashMap;

use camino::Utf8PathBuf;
use clap::Parser;
use clap::Subcommand;
use glob_match::glob_match;
use miette::Context;
use miette::IntoDiagnostic;
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

    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    Verify {
        /// List of files to check their licence on
        files: Vec<Utf8PathBuf>,
    },
}

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    languages: HashMap<String, Vec<String>>,
}

fn default_languages() -> HashMap<String, Vec<String>> {
    [
        (String::from("bash"), vec![String::from("*.sh")]),
        (String::from("rust"), vec![String::from("*.rs")]),
        (String::from("toml"), vec![String::from("*.toml")]),
    ]
    .into()
}

struct Language {
    name: String,
    library: libloading::Library,
    language_fn: LanguageFn,
}

fn main() -> miette::Result<()> {
    tracing_subscriber::fmt::fmt()
        .pretty()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let langs = load_languages(&args)?;

    let config = Config {
        languages: default_languages(),
    };

    match args.command {
        Command::Verify { files } => {
            for file in files {
                debug!("Checking {}", file);
                let language = config.languages.iter().find_map(|(name, globs)| {
                    globs
                        .iter()
                        .any(|glob| glob_match(glob, file.file_name().unwrap()))
                        .then_some(name)
                });

                let Some(name) = language else {
                    info!("Could not determine language for {}", file);
                    continue;
                };

                let Some(language) = langs.get(name) else {
                    info!(
                        "Found language {} but no tree-sitter grammar exists for it",
                        name
                    );
                    continue;
                };

                let grammar = tree_sitter::Language::new(language.language_fn);

                let mut parser = tree_sitter::Parser::new();
                parser.set_language(&grammar).into_diagnostic()?;

                let text = std::fs::read_to_string(file).into_diagnostic()?;
                let Some(tree) = parser.parse(&text, None) else {
                    miette::bail!("Could not parse file")
                };

                let mut cursor = tree.walk();

                for child in tree.root_node().children(&mut cursor) {
                    if child.grammar_name() == "comment" {
                        info!(
                            "Found comment: '{}'",
                            child.utf8_text(text.as_bytes()).into_diagnostic()?
                        );
                    }
                }
            }
        }
    }

    Ok(())
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
        name: lang_name.to_string(),
        library,
        language_fn,
    })
}
