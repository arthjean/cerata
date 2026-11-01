# ADR 0004 : coloration du code, tree-sitter-highlight dans un crate `cerata_render`

> Statut : **décidé** le 2026-11-01. Story : US-014. Plan : `PLAN.md` §3 (« un crate de rendu ne sera extrait qu'à l'arrivée de ses dépendances lourdes »).

## Contexte

US-014 exige des grammaires tree-sitter pour au moins 30 langages dans les fences. Ces grammaires sont du C compilé : les mettre dans `cerata_markdown` alourdirait le crate pur que les tests et le futur client Android consomment. Le plan prévoyait d'extraire un crate de rendu à l'arrivée de telles dépendances : elles arrivent.

## Options

- **arborium 2.18.2** (MIT ou Apache-2.0, une feature par langage, requêtes soignées) : écarté. Il embarque `arborium-tree-sitter`, un fork qui déclare `links = "tree-sitter"`, incompatible avec `tree-sitter` 0.27.1 déjà dans le graphe du workspace (dev-dépendance du spike parser de US-003, `examples/parser_spike`). Cargo refuse deux paquets liant la même bibliothèque native. L'adopter imposerait de supprimer le spike, hors périmètre.
- **tree-sitter-highlight 0.27.1 et les crates de grammaire officielles** (MIT) : retenu. Même version de tree-sitter que le spike, une dépendance par langage, requêtes `HIGHLIGHTS_QUERY` livrées par chaque crate.

## Décision

Nouveau crate `crates/cerata_render`, sans GPUI (ajouté à `scripts/check-core-deps.sh`), dépendant de `cerata_markdown`. `highlight(info, code)` renvoie les spans dans les rôles de couleur de Writer (`generalSyntaxHighlights`), ou `None` pour un langage inconnu (rendu monospace sans couleur, sans erreur) ; `highlight_code_blocks(state)` donne les spans de toutes les fences en offsets du document. 34 langages : JavaScript, TypeScript, TSX, Python, Bash, JSON, Rust, HTML, CSS, YAML, SQL, Go, Java, C, C++, C#, PHP, Ruby, Swift, TOML, XML, diff, Lua, Scala, Haskell, R, Dart, Elixir, OCaml, Zig, Make, Nix, Elm, regex. Julia et Kotlin sont absents : leurs crates ne livrent pas de requête de coloration. Les configurations se compilent au premier usage de chaque langage.

## Conséquences

- Quatre crates au lieu de trois. Le coût de compilation des grammaires ne touche que `cerata_render` et l'application.
- Quand le spike de US-003 sera supprimé, arborium redeviendra possible ; le changement se limite à `src/highlight.rs`.
- Pas d'injections de langage (CSS dans HTML) : non requises par la story.

## Non prouvé

- Qualité visuelle de la coloration : à juger dans le rendu GPUI (EP-003).
