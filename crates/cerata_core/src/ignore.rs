use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct ScopedGitignore {
    pub(crate) scope: PathBuf,
    pub(crate) matcher: Gitignore,
}

impl ScopedGitignore {
    pub(crate) fn read(dir: &Path) -> Option<Self> {
        let candidate = dir.join(".gitignore");
        if !candidate.is_file() {
            return None;
        }
        let (matcher, error) = Gitignore::new(&candidate);
        if let Some(error) = error {
            log::warn!("partially invalid {}: {error}", candidate.display());
        }
        Some(Self {
            scope: dir.to_path_buf(),
            matcher,
        })
    }

    fn decide(&self, path: &Path, is_dir: bool) -> Match<()> {
        if !path.starts_with(&self.scope) {
            return Match::None;
        }
        match self.matched(path, is_dir) {
            Match::Ignore(_) => Match::Ignore(()),
            Match::Whitelist(_) => Match::Whitelist(()),
            Match::None => Match::None,
        }
    }

    fn matched<'a>(&'a self, path: &Path, is_dir: bool) -> Match<&'a ignore::gitignore::Glob> {
        self.matcher.matched_path_or_any_parents(path, is_dir)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RootRules {
    exclude: Option<ScopedGitignore>,
    global: Option<ScopedGitignore>,
}

impl RootRules {
    fn load(root: &Path, global_file: Option<&Path>) -> Self {
        let exclude = build_rooted(root, &root.join(".git").join("info").join("exclude"));
        let global = global_file.and_then(|file| build_rooted(root, file));
        Self { exclude, global }
    }
}

fn build_rooted(root: &Path, file: &Path) -> Option<ScopedGitignore> {
    if !file.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(root);
    if let Some(error) = builder.add(file) {
        log::warn!("partially invalid {}: {error}", file.display());
    }
    match builder.build() {
        Ok(matcher) if !matcher.is_empty() => Some(ScopedGitignore {
            scope: root.to_path_buf(),
            matcher,
        }),
        Ok(_) => None,
        Err(error) => {
            log::warn!("ignoring {}: {error}", file.display());
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceIgnore {
    root: PathBuf,
    gitignores: Vec<ScopedGitignore>,
    root_rules: RootRules,
}

impl WorkspaceIgnore {
    pub fn bootstrap(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            gitignores: Vec::new(),
            root_rules: RootRules::default(),
        }
    }

    pub fn load(root: &Path) -> Self {
        Self::load_with_global(root, global_gitignore_path().as_deref())
    }

    pub fn load_with_global(root: &Path, global_file: Option<&Path>) -> Self {
        let builder = IgnoreBuilder::new(root, global_file);
        let mut stack = vec![(root.to_path_buf(), builder.chain_for_root())];
        while let Some((dir, chain)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let path = entry.path();
                if builder.excludes_with(&chain, &path, true) {
                    continue;
                }
                let child_chain = builder.enter(&chain, &path);
                stack.push((path, child_chain));
            }
        }
        builder.finish()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        if has_safety_component(path) {
            return true;
        }
        decide_rules(self.gitignores.iter(), &self.root_rules, path, is_dir)
    }

    pub fn excludes(&self, path: &Path, is_dir: bool) -> bool {
        is_hidden_below(&self.root, path) || self.is_ignored(path, is_dir)
    }
}

pub fn is_gitignore_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == ".gitignore")
}

pub fn global_gitignore_path() -> Option<PathBuf> {
    ignore::gitignore::gitconfig_excludes_path()
}

fn has_safety_component(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str();
        name == "node_modules" || name == ".git"
    })
}

fn is_hidden_below(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    relative
        .components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
}

fn decide_rules<'a>(
    gitignores: impl Iterator<Item = &'a ScopedGitignore>,
    root_rules: &'a RootRules,
    path: &Path,
    is_dir: bool,
) -> bool {
    let root_level = root_rules.exclude.iter().chain(root_rules.global.iter());
    for scoped in gitignores.chain(root_level) {
        match scoped.decide(path, is_dir) {
            Match::Ignore(()) => return true,
            Match::Whitelist(()) => return false,
            Match::None => {}
        }
    }
    false
}

pub(crate) struct IgnoreBuilder {
    root: PathBuf,
    root_rules: RootRules,
    collected: parking_lot::Mutex<Vec<ScopedGitignore>>,
}

pub(crate) type GitignoreChain = std::sync::Arc<Vec<std::sync::Arc<ScopedGitignore>>>;

impl IgnoreBuilder {
    pub(crate) fn new(root: &Path, global_file: Option<&Path>) -> Self {
        Self {
            root: root.to_path_buf(),
            root_rules: RootRules::load(root, global_file),
            collected: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn chain_for_root(&self) -> GitignoreChain {
        self.enter(&GitignoreChain::default(), &self.root.clone())
    }

    pub(crate) fn enter(&self, parent: &GitignoreChain, dir: &Path) -> GitignoreChain {
        match ScopedGitignore::read(dir) {
            Some(scoped) => {
                self.collected.lock().push(scoped.clone());
                let mut chain = Vec::with_capacity(parent.len() + 1);
                chain.push(std::sync::Arc::new(scoped));
                chain.extend(parent.iter().cloned());
                std::sync::Arc::new(chain)
            }
            None => parent.clone(),
        }
    }

    pub(crate) fn excludes_with(&self, chain: &GitignoreChain, path: &Path, is_dir: bool) -> bool {
        is_hidden_below(&self.root, path)
            || has_safety_component(path)
            || decide_rules(
                chain.iter().map(|scoped| scoped.as_ref()),
                &self.root_rules,
                path,
                is_dir,
            )
    }

    pub(crate) fn finish(self) -> WorkspaceIgnore {
        let mut gitignores = self.collected.into_inner();
        gitignores.sort_by(|a, b| {
            b.scope
                .components()
                .count()
                .cmp(&a.scope.components().count())
                .then_with(|| a.scope.cmp(&b.scope))
        });
        WorkspaceIgnore {
            root: self.root,
            gitignores,
            root_rules: self.root_rules,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn load(dir: &TempDir) -> WorkspaceIgnore {
        WorkspaceIgnore::load_with_global(dir.path(), None)
    }

    #[test]
    fn node_modules_is_always_ignored() {
        let dir = TempDir::new().unwrap();
        let ignore = load(&dir);
        assert!(ignore.is_ignored(&dir.path().join("node_modules"), true));
        assert!(ignore.is_ignored(&dir.path().join("node_modules").join("foo.md"), false));
    }

    #[test]
    fn bootstrap_enforces_only_the_safety_net() {
        let dir = TempDir::new().unwrap();
        let ignore = WorkspaceIgnore::bootstrap(dir.path());
        assert!(ignore.is_ignored(&dir.path().join("node_modules"), true));
        assert!(ignore.is_ignored(&dir.path().join(".git").join("HEAD"), false));
        assert!(!ignore.is_ignored(&dir.path().join("drafts"), true));
        assert!(!ignore.is_ignored(&dir.path().join("readme.md"), false));
    }

    #[test]
    fn git_dir_is_always_ignored() {
        let dir = TempDir::new().unwrap();
        let ignore = load(&dir);
        assert!(ignore.is_ignored(&dir.path().join(".git").join("HEAD"), false));
    }

    #[test]
    fn root_gitignore_is_applied() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join(".gitignore"), "dist/\n*.log\n");
        touch(&dir.path().join("readme.md"), "# Readme");
        let ignore = load(&dir);
        assert!(ignore.is_ignored(&dir.path().join("dist"), true));
        assert!(ignore.is_ignored(&dir.path().join("dist").join("bundle.js"), false));
        assert!(ignore.is_ignored(&dir.path().join("error.log"), false));
        assert!(!ignore.is_ignored(&dir.path().join("readme.md"), false));
    }

    #[test]
    fn nested_star_rule_stays_scoped_to_its_directory() {
        let dir = TempDir::new().unwrap();
        touch(
            &dir.path().join("hooks").join("_").join(".gitignore"),
            "*\n",
        );
        touch(&dir.path().join("readme.md"), "# Readme");
        touch(&dir.path().join("docs").join("guide.md"), "# Guide");
        touch(
            &dir.path().join("hooks").join("_").join("hook.sh"),
            "#!/bin/sh",
        );
        let ignore = load(&dir);
        assert!(!ignore.is_ignored(&dir.path().join("readme.md"), false));
        assert!(!ignore.is_ignored(&dir.path().join("docs"), true));
        assert!(!ignore.is_ignored(&dir.path().join("docs").join("guide.md"), false));
        assert!(ignore.is_ignored(&dir.path().join("hooks").join("_").join("hook.sh"), false));
    }

    #[test]
    fn nested_gitignore_is_applied() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join(".gitignore"), "# empty\n");
        touch(&dir.path().join("docs").join(".gitignore"), "drafts/\n");
        touch(
            &dir.path().join("docs").join("drafts").join("wip.md"),
            "# wip",
        );
        touch(&dir.path().join("docs").join("final.md"), "# final");
        let ignore = load(&dir);
        assert!(ignore.is_ignored(&dir.path().join("docs").join("drafts"), true));
        assert!(ignore.is_ignored(
            &dir.path().join("docs").join("drafts").join("wip.md"),
            false
        ));
        assert!(!ignore.is_ignored(&dir.path().join("docs").join("final.md"), false));
    }

    #[test]
    fn gitignore_file_itself_stays_visible_to_the_rules() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join(".gitignore"), "dist/\n");
        let ignore = load(&dir);
        assert!(!ignore.is_ignored(&dir.path().join(".gitignore"), false));
        assert!(ignore.excludes(&dir.path().join(".gitignore"), false));
    }

    #[test]
    fn unignored_paths_return_false() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join("notes").join("hello.md"), "# hi");
        touch(&dir.path().join(".gitignore"), "dist/\n");
        let ignore = load(&dir);
        assert!(!ignore.is_ignored(&dir.path().join("notes"), true));
        assert!(!ignore.is_ignored(&dir.path().join("notes").join("hello.md"), false));
    }

    #[test]
    fn is_gitignore_path_matches_filename() {
        assert!(is_gitignore_path(Path::new("/a/b/.gitignore")));
        assert!(is_gitignore_path(Path::new(".gitignore")));
        assert!(!is_gitignore_path(Path::new("/a/b/gitignore.txt")));
        assert!(!is_gitignore_path(Path::new("/a/b/.gitignore.bak")));
    }

    #[test]
    fn git_info_exclude_is_applied() {
        let dir = TempDir::new().unwrap();
        touch(
            &dir.path().join(".git").join("info").join("exclude"),
            "private/\n",
        );
        touch(&dir.path().join("private").join("secret.md"), "x");
        let ignore = load(&dir);
        assert!(ignore.excludes(&dir.path().join("private").join("secret.md"), false));
    }

    #[test]
    fn global_gitignore_is_applied() {
        let dir = TempDir::new().unwrap();
        let global = TempDir::new().unwrap();
        let global_file = global.path().join("global-ignore");
        touch(&global_file, "*.scratch.md\n");
        let ignore = WorkspaceIgnore::load_with_global(dir.path(), Some(&global_file));
        assert!(ignore.excludes(&dir.path().join("a").join("b.scratch.md"), false));
        assert!(!ignore.excludes(&dir.path().join("a").join("b.md"), false));
    }

    #[test]
    fn hidden_components_below_the_root_are_excluded() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".vault");
        fs::create_dir_all(&root).unwrap();
        let ignore = WorkspaceIgnore::load_with_global(&root, None);
        assert!(!ignore.excludes(&root.join("note.md"), false));
        assert!(ignore.excludes(&root.join(".DS_Store"), false));
        assert!(ignore.excludes(&root.join("sub").join(".hidden.md"), false));
        assert!(ignore.excludes(&root.join(".obsidian").join("app.md"), false));
        assert!(ignore.excludes(&root.join(".cerata").join("config"), false));
    }

    #[test]
    fn whitelist_in_a_deeper_gitignore_wins() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join(".gitignore"), "*.md\n");
        touch(&dir.path().join("keep").join(".gitignore"), "!*.md\n");
        let ignore = load(&dir);
        assert!(ignore.excludes(&dir.path().join("a.md"), false));
        assert!(!ignore.excludes(&dir.path().join("keep").join("b.md"), false));
    }

    #[test]
    fn paths_outside_the_root_never_panic() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join(".gitignore"), "*.log\n");
        touch(
            &dir.path().join(".git").join("info").join("exclude"),
            "x/\n",
        );
        let ignore = load(&dir);
        let elsewhere = TempDir::new().unwrap();
        assert!(!ignore.excludes(&elsewhere.path().join("a.md"), false));
    }

    #[test]
    fn rules_inside_an_ignored_directory_are_never_collected() {
        let dir = TempDir::new().unwrap();
        touch(&dir.path().join(".gitignore"), "build/\n");
        touch(&dir.path().join("build").join(".gitignore"), "!*\n");
        let ignore = load(&dir);
        assert!(ignore.excludes(&dir.path().join("build").join("out.md"), false));
    }
}
