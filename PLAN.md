# Plan de portage : writer-computer vers Rust + GPUI

> Statut : plan vérifié (deux vagues d'exploration), rien d'implémenté. Mis à jour le 2026-10-31.
> Base de référence : `C:\dev\writer-computer` au commit `79b744c` (dernier commit le 2026-10-28).
> GPUI de référence : `C:\dev\zed` au commit `ba63a022c9` (2026-10-31).
> Nom de travail : Cerata. Les durées sont des ordres de grandeur, à recalibrer après la phase 0.

## 0. Verdict

- **Pile retenue : GPUI + Rust en direct**, Zed épinglé par rev git comme dans Paneflow. GPUIX et Zig sont écartés (§3.1).
- **Licence : GPL-3.0-or-later**, comme Paneflow. Retravailler le code ensuite ne change pas son statut d'œuvre dérivée (§3.2).
- **Ampleur.** ~33 000 lignes de source (~23 000 TypeScript/React/CSS, ~10 000 Rust) et ~8 600 lignes de tests (716 tests TS, 168 tests Rust). ~70 % du backend Rust se porte presque tel quel. Le frontend est à réécrire : GPUI n'a ni DOM ni CodeMirror.
- **Moteur d'édition dédié, confirmé par le code de Zed.** L'éditeur de Zed impose une hauteur de ligne uniforme, et ses styles par plage n'incluent pas la taille de police : il ne peut pas grossir les titres. Le text system de GPUI met en forme une ligne avec une seule taille de police : un titre, qui occupe une ligne entière, est donc possible dans un moteur maison. C'est ~45 % de l'effort, et la pièce la plus innovante du projet.
- **Mobile.** GPUI a un backend iOS officiel depuis le 2026-10-14 (UIKit, CoreText, Metal, saisie native), encore incomplet. Rien pour Android.
- **Effort** (solo, plein temps, déjà expert GPUI via Paneflow) : 6 à 9 mois pour la parité, 4 à 6 mois pour un MVP utilisable au quotidien (sans Mermaid, math ni HTML).
- **Décisions de la phase 0** : moteur d'édition (spike go/no-go), parser, et une question de licence liée à l'App Store iOS, qui décide aussi de la réutilisation des crates GPL de Zed (§3.2).

## 1. Méthode, couverture et vérification

Deux vagues de sous-agents en parallèle, puis vérification par sondage de chaque rapport dans le code ou les registres officiels.

**Vague 1 : writer-computer et écosystème**

| Périmètre | Couverture | Vérifié par moi | Corrections |
|---|---|---|---|
| Backend `src-tauri/src/**` | 100 % | 44 commandes IPC (`lib.rs:575-623`), filtres `.md` incohérents, `rfind('/')` (`search.rs:112`), détection CLI (`main.rs:21-29`) | aucune |
| `lib/prosemark-core/**` + helpers | 100 % | licence MIT (J. Simon Richard), absence de parseur `#tag`, regex du slug | fausse alerte : la plage du slug commence bien à U+2000 |
| `components/editor-area/**` | 52/52 fichiers | recherche littérale insensible à la casse (`editor-search-store.ts:62-74`), canvas Mermaid 480 px, LRU 50 et 200, limite image 5 Mo | aucune |
| Shell : stores, hooks, sidebar, palette, réglages, lib | ~95 % en deux passes | rechargement disque sans contrôle « dirty », presets de thème jamais chargés | 43 réglages et non 39 |
| Docs, SPECs, TODOS, tests | docs et TODOS complets ; SPECs et tests par titres | raccourcis (`docs/keyboard-shortcuts.md`), release arm64 seulement (`docs/releasing.md:90`) | aucune |
| Crates de rendu et distribution | web + API crates.io/npm | pele 1.13.0 MIT (obsidianmd), zéro dépendance ; options de pulldown-cmark 0.13.6 ; extensions de tree-sitter-md | arborium en 2.18.2 |

**Vague 2 : choix de pile, à partir du code source**

| Périmètre | Source | Vérifié par moi | Corrections |
|---|---|---|---|
| Capacités plateforme de GPUI | `C:\dev\zed\crates\gpui*` | `TextRun` sans `font_size` (`gpui/src/text_system.rs:1070-1083`), `gpui_ios` ajouté upstream par la PR #63068 le 2026-10-14 (~3 700 lignes), aucun TSF dans `gpui_windows`, pas de crate Android | **la vague 1 affirmait « aucun support iOS » : faux** |
| Éditeur de Zed | `crates/editor`, `multi_buffer`, `text`, `rope`, `sum_tree` | hauteur de ligne uniforme (`element.rs:2114`, `:3509` arrondit les blocs à un multiple de la hauteur de ligne), `HighlightStyle` sans taille de police (`gpui/src/style.rs:575-605`), licences, dépendances légères de `text` et `rope` | aucune |
| GPUIX | `C:\dev\gpuix` | version 0.11.0, sous-module `remorses/zed` branche `gpuix` au commit `f77e5cf` | Bun/Node + napi, pas Hermes (Hermes n'est qu'un prototype) |
| Zig | web | Zig 0.17.0 sorti le 2026-10-01, master 0.18.0-dev ; Ghostty 1.4.0 « drop any roadmap for Windows support in the Ghostty GUI » | aucune |
| Paneflow (contexte) | `C:\dev\paneflow` | GPUI par rev git `37faef81f7…`, `[patch.crates-io]` alignés sur Zed, libghostty (Zig) intégré par C ABI, licence `GPL-3.0-or-later` | aucune |

Non lu en détail : le corps de la plupart des SPECs et des tests (titres seulement, qui encodent les invariants), quelques fichiers sidebar (`sidebar-navigator.tsx` interne, `flatten-tree.ts`, `font-stack.ts`). Hors périmètre : `apps/website` ; `apps/playground` sert d'oracle de test (§9).

## 2. Anatomie de l'application source

Writer est une app Tauri v2 : frontend React 19 + Zustand + CodeMirror 6 avec une couche « prosemark » de live preview, backend Rust exposant 44 commandes IPC et 7 événements. Concepts à préserver :

- **Identité de workspace** `{root, epoch}` portée par chaque événement et chaque résultat asynchrone : un résultat d'un workspace périmé est jeté (`state.rs:184`). C'est le mécanisme anti-race central.
- **Schéma de réglages unique** `shared/settings.schema.json` (43 clés : types, défauts, liaisons CSS), lu par Rust et par le frontend. Config `key = value` qui préserve les commentaires, en trois couches : défauts, global, workspace (`{root}/.writer/config`).
- **Onglets** avec historique propre, au moins un onglet toujours présent (launcher), types de page `file`, `launcher`, `settings`.
- **Sauvegarde** : au plus une écriture par seconde et par fichier, jamais deux en parallèle, temp + rename, ré-écriture si le texte a changé pendant l'écriture (`lib/save.ts`).
- **Règle de live preview** : si une plage de sélection touche un nœud (bornes incluses, `utils.ts:126-132`), le nœud montre sa source ; sinon son markup est masqué.

## 3. Décisions

### 3.1 Choix de la pile

| Critère | GPUI + Rust | GPUIX | Zig |
|---|---|---|---|
| Desktop 3 OS | oui : Metal, DirectX, wgpu (Wayland + X11) | oui, mais pas de multi-fenêtre ni de menus d'app déclarés | aucune GUI Zig ne gère mise en forme du texte, IME et accessibilité ; reste une UI native par OS, donc 3 UI |
| Moteur d'édition (45 % de l'effort) | élément GPUI maison | élément Rust maison quand même (CodeMirror ne se porte pas) | à écrire 3 fois (TextKit 2, GTK4, WinUI) ou à bâtir sur un renderer maison |
| IME | macOS, Wayland, X11, iOS ; Windows en IMM32 seulement | hérite de GPUI | fourni par chaque OS en UI native |
| Accessibilité | AccessKit sur les 4 backends desktop | hérite de GPUI | native par OS |
| Mobile | iOS officiel depuis 2026-10-14 ; pas d'Android | iOS seulement à l'état de plan | iOS en Tier 3, Android absent de la table des cibles 0.17 |
| Stabilité | pré-1.0, ruptures fréquentes | 0.11.0, mainteneur unique, fork de Zed | 0.17.0 très cassante, ZLS cassé avec le nouveau `build.zig` |
| Coût runtime | binaire natif | runtime JS en plus : 80 Mo au repos, binaire Bun de 82 Mo | binaire natif |
| Ton expérience | Paneflow : workflow d'épinglage et de patchs déjà rodé | nulle | libghostty intégré dans Paneflow, pas d'app GUI Zig |

**Décision : GPUI + Rust en direct.**

- **GPUIX** n'aiderait que pour le shell (onglets, sidebar, réglages), la partie la moins chère. Il ajoute un second runtime au cœur des promesses du produit (démarrage, mémoire, taille), il manque le multi-fenêtre, que Writer a déjà, et il dépend d'un seul mainteneur. Le moteur d'édition resterait un élément Rust.
- **Zig** pousse vers le modèle de Ghostty : un cœur en C ABI et une UI native par OS. Ghostty lui-même vient d'abandonner sa GUI Windows (1.4.0, 2026-10-27) à cause de ce coût. Pour Cerata, dont la valeur est justement l'UI d'édition, ce serait l'écrire trois fois. Zig reste possible plus tard pour un composant borné derrière un C ABI, comme libghostty dans Paneflow, mais seulement face à un besoin mesuré.
- **L'innovation est ailleurs que dans le langage** : aucun éditeur GPUI ne fait de live preview avec des hauteurs de ligne variables, pas même celui de Zed. Ce moteur peut devenir un crate réutilisable par l'écosystème GPUI, ce qui donne aussi au projet un levier de diffusion.

Le portage Tauri, plus rapide (3 à 6 semaines, estimation non testée), est écarté par choix : l'objectif est de construire mieux, pas seulement de porter.

### 3.2 Licence

**Cerata sera en GPL-3.0-or-later.** Le fait de retravailler ensuite certaines parties ne change rien : la GPLv3 appelle « modifier » le fait de « copier ou adapter tout ou partie de l'œuvre » (§0), et une traduction en Rust est une adaptation. Toute œuvre basée sur le programme doit être distribuée sous GPL dans son ensemble (§5c), avec des mentions visibles des modifications et de leur date (§5a). Le fichier `LICENSE` de Writer est le texte brut de la GPLv3, sans en-tête SPDX.

Obligations : conserver la notice de copyright de Writer et la licence MIT de `prosemark-core` (J. Simon Richard), indiquer les fichiers modifiés, publier les sources avec chaque binaire.

Conséquence utile : les crates GPL de Zed (`text`, `rope`, `editor`, `markdown` : toutes en GPL-3.0-or-later) deviennent utilisables. `sum_tree`, `gpui` et `gpui_ios` sont en Apache-2.0.

**Le vrai point de décision est l'App Store iOS.** Les conditions de l'App Store sont réputées incompatibles avec la GPL (position de la FSF, retrait de VLC en 2011). Une exception GPLv3 (§7) ne peut venir que des titulaires des droits : toi pour ton code, Joel (421 des 425 commits de Writer) pour le code porté. Réutiliser `text` ou `rope` de Zed ajouterait Zed Industries à cette liste, et cette exception-là n'arrivera pas. D'où la règle de la phase 0 :

1. Écrire à Joel pour demander une exception App Store pour le code porté.
2. Exception accordée : ne dépendre d'aucun crate GPL de Zed (`sum_tree` en Apache-2.0 suffit pour construire le buffer).
3. Exception refusée, ou App Store sans importance : réutiliser `text` et `rope` de Zed, plus rapide et éprouvé.

### 3.3 Nom et identité

Ne réutiliser ni « Writer », ni « Writer Computer », ni le logo, ni les contenus du site. Identifiant d'app et dossiers de config propres, pour ne pas entrer en collision avec une installation de Writer. Un import des sessions et de la config de Writer pourra venir plus tard (formats documentés dans `config.rs` et `workspace.rs`).

### 3.4 Moteur d'édition

**Option B (fork de l'éditeur Zed) écartée, preuves à l'appui** :
- Les folds peuvent masquer une plage quelconque et afficher n'importe quel élément (`fold_map.rs:27-39`), mais révéler la source au contact de la sélection n'existe pas : il faudrait plier et déplier à chaque changement de sélection.
- Les blocs `Replace` rendent n'importe quel élément, mais seulement sur des lignes entières, et leur hauteur est arrondie à un multiple de la hauteur de ligne (`block_map.rs:159-172`, `element.rs:3509`).
- La hauteur de ligne est unique : chaque rangée est placée à `row × line_height` (`element.rs:2114`), et la wrap map est construite avec une seule police et une seule taille.
- `HighlightStyle` ne porte ni taille ni famille de police.
- L'éditeur dépend de `project`, `workspace`, `lsp`, `settings`, `language`, `git`, `theme`, `ui`.

Option A (éditeur de gpui-component) : non évaluée en détail, mais même famille d'éditeur de code. Elle n'est plus prioritaire.

**Option C retenue : élément GPUI dédié.** Contraintes de GPUI vérifiées :
- **Une taille de police par ligne** : `layout_line(text, font_size, runs)` prend une seule taille (`gpui_apple/src/text_system.rs:212`, `gpui_wgpu/src/cosmic_text_system.rs:273`). Les titres occupent une ligne entière, donc c'est compatible. Mais aucun changement de taille en milieu de ligne : le code inline, les exposants et indices gardent la taille de la ligne.
- Graisse, style et famille varient par run (`Font` dans `TextRun`).
- Le retour à la ligne est commun à tous les OS (`LineWrapper`, `gpui/src/text_system/line_wrapper.rs`).
- L'IME passe par `EntityInputHandler`, sur le modèle du `<textarea>` de GPUIX et de l'éditeur de Zed.

Conception :
1. **Buffer** : `text` + `rope` de Zed (dépendances légères : `clock`, `collections`, `rope`, `sum_tree`, `gpui_util`, aucune dépendance projet ni workspace), ou rope maison sur `sum_tree` selon la règle du §3.2. Transactions, filtres de transaction (règles de caret des listes), undo groupé dans le temps.
2. **Arbre syntaxique** incrémental derrière un type `MarkdownTree` interchangeable (§3.5).
3. **Règles de décoration** : une fonction pure `decorate(tree, selections, drag_frozen) -> Vec<Decoration>` avec les variantes `Hide`, `Invisible`, `Replace(widget)`, `Block(widget)`, `LineStyle`, `MarkStyle`. Toute la logique de prosemark-core et d'editor-area s'exprime ici, testable sans GPUI.
4. **Display map à hauteurs variables** : correspondance offsets buffer et positions affichées, retour à la ligne par ligne avec sa propre taille, blocs à la hauteur exacte (sans arrondi), et une height map en arbre de sommes (`sum_tree`) pour le défilement et le hit-testing. C'est la vraie différence avec Zed.
5. **Rendu** : un `Element` virtualisé (lignes visibles + marge), widgets en `AnyElement` mesurés.

Une partie du code CodeMirror disparaît, car elle ne sert qu'à contourner la virtualisation du DOM (`offscreenSelectionHighlight`, `viewport-parse`, rendered ranges, `posAtDOM`, `scrollSnapshot`).

### 3.5 Parser Markdown

Writer s'appuie sur 8 règles Lezer personnalisées : frontmatter seulement à l'offset 0, math `$…$` façon Pandoc et `$$…$$`, destinations de lien avec espaces, liens imbriqués traités comme texte, setext où un seul `-` ne fait pas un H2, escape étendu à tout caractère, tirets (`--`, `---`), emoji `:mot:`. Les wiki-links passent par une regex. Il n'y a aucun parseur de `#tag`.

| Option | Forces | Faiblesses |
|---|---|---|
| **tree-sitter-md 0.5.4 (recommandé)** | incrémental ; granularité proche de Lezer (marqueurs d'emphase, de titre, de liste, destinations de lien, à confirmer) | LATEX et WIKI_LINK ne s'activent qu'en régénérant la grammaire (vendoring) ; les règles setext et liens avec espaces demandent de modifier le scanner C |
| pulldown-cmark 0.13.6 (repli) | math, wiki-links, frontmatter YAML et GFM natifs ; reparse complet très rapide | pas incrémental ; pas de nœuds pour les marqueurs (`**`, `](`, URL), à reconstruire depuis le texte |
| comrak 0.59 | AST le plus riche | plus lent, positions ligne/colonne, mêmes manques de marqueurs |

Critères du spike : reparse en moins de 2 ms sur la note de 3 000 items du test e2e `list-enter-exit`, moins de 16 ms sur une note de 1 Mo, couverture des 8 règles sur un corpus de fixtures.

**Décision (US-003, 2026-10-31, `docs/adr/0002-markdown-parser.md`) : pulldown-cmark 0.13.6 est retenu, avec reconstruction des marqueurs.** La recommandation ci-dessus est écartée sur mesures : le seul arbre de blocs de tree-sitter-md coûte 3,3 ms au p50 par édition sur la liste de 3 000 items, au-delà du budget de 2 ms, quand pulldown-cmark reparse tout en 0,73 ms (p50) et 7,9 ms sur 1 Mo. L'arbre de US-010 est donc recalculé à chaque édition, pas mis à jour de façon incrémentale. Amendement US-010 (2026-11-01) : l'inline est un portage de `@lezer/markdown`, et le parse complet dépassant le budget avec l'arbre réel, seule une fenêtre de blocs autour de l'édition est reparsée (0,29 ms au p99 sur la liste, 2,0 ms sur 1 Mo), voir l'amendement de l'ADR 0002. Buffer : `docs/adr/0003-buffer.md`.

### 3.6 Mermaid et math

**Mermaid**, par ordre de préférence :
1. `mermaid-svg` 0.7.0 (pur Rust, MIT, 23 types annoncés, 6 versions cassantes depuis mai 2026), sortie SVG dessinée via `svg()` de GPUI (usvg).
2. Pele (MIT, Obsidian, sans dépendance) dans `rquickjs` 0.14, s'il n'exige pas de DOM pour mesurer le texte (non vérifié).
3. Dernier recours : bloc de code coloré.

**Math**, par ordre de préférence :
1. LaTeX converti en math Typst (`mitex` 0.2.4, dernière version 2024-06 : risque de maintenance), puis `typst` 0.15.1 + `typst-svg`.
2. MathJax en sortie SVG (adaptateur sans navigateur) dans `rquickjs`, qui partagerait le moteur JS avec Pele.
3. `pulldown-latex` 0.8 ne sort que du MathML : à écarter.

Règle : si un moteur JS embarqué est retenu pour l'un, l'utiliser pour les deux, dans un seul bac à sable de rendu hors du thread UI. Ces rendus n'arrivent qu'en phase 4.

### 3.7 Mobile

- **iOS** : `gpui_ios` est un vrai portage (UIKit, CoreText, Metal, `UITextInput`, cycle de vie, avertissements mémoire). Manquent encore : sélecteurs de fichiers, `open_with_system`, presse-papiers d'images, menus, glisser-déposer, accessibilité (`gpui_ios/src/ios/platform.rs:309-369`). Le même crate `cerata` peut donc viser iOS avec une mise en page adaptée, une fois ces trous comblés upstream ou contournés.
- **Android** : aucun backend. Le trait de plateforme mentionne Android (`gpui/src/platform.rs`), signe d'un travail en cours, sans garantie. Repli : UI native Compose sur les crates sans GPUI via UniFFI.
- Les crates `cerata_core` et `cerata_markdown` restent sans GPUI : c'est requis pour Android et pour des tests purs.
- Restent hors plan : la synchronisation des notes (iCloud, Syncthing, git, service maison), l'accès aux fichiers dans les bacs à sable mobiles et la licence App Store (§3.2).

Piste à creuser plus tard : `text` de Zed est un buffer CRDT (horloges de Lamport, opérations fusionnables). Il pourrait fonder une synchro desktop-mobile sans conflit, si la règle du §3.2 permet de l'utiliser.

## 4. Architecture cible

```
cerata/
  Cargo.toml              workspace ; GPUI par rev git exact + [patch.crates-io] alignés (modèle Paneflow)
  crates/
    cerata_core/          sans GPUI : workspace, fs, ignore, index, recherche, watcher,
                          réglages, sessions, récents, images, cibles d'ouverture, CLI
    cerata_markdown/      sans GPUI : buffer, transactions, parser, règles de décoration,
                          commandes d'édition, wiki-links, frontmatter, slugs, statistiques
    cerata/               GPUI : fenêtres, élément éditeur, sidebar, onglets, palette,
                          réglages, thèmes, intégration OS
  assets/                 polices, icônes, thèmes, schéma de réglages
```

- Trois crates au départ. Exigence qui justifie la séparation : Android et tests purs. Un crate de rendu (Mermaid, math, coloration) ne sera extrait qu'à l'arrivée de ses dépendances lourdes.
- **Épinglage** : reprendre le workflow de Paneflow (rev git exact, forks `[patch.crates-io]` synchronisés, feature `font-kit` obligatoire sur macOS).
- **Composants d'UI** : gpui-component épingle `gpui-pre =0.3.10` ; il n'est utilisable que si cette version correspond au rev choisi. Sinon, partir des composants déjà écrits pour Paneflow.
- Le moteur d'édition est conçu comme un crate séparable dès le début, sans dépendance vers le reste de Cerata.

| Writer (Tauri) | Cerata (GPUI, vérifié dans `C:\dev\zed`) |
|---|---|
| 44 commandes IPC + `emit_to(label)` | appels directs + entités et événements GPUI par fenêtre ; garder `{root, epoch}` |
| Stores Zustand | `Entity<EditorState>`, `Entity<WorkspaceState>`, réglages en état global |
| plugin-dialog | dialogues natifs sur macOS et Windows (IFileDialog), portail XDG via ashpd sous Linux |
| plugin-clipboard-manager | presse-papiers GPUI, images PNG sur les 4 backends desktop (texte seulement sur iOS) |
| plugin-opener | `open_url` / `open_with_system` de GPUI |
| plugin-store (`preferences.json`, fichiers épinglés) | fichier JSON via serde |
| plugin-updater (minisign + `latest.json`) | Velopack ou cargo-packager-updater |
| plugin-single-instance | socket local ou named pipe (GPUI fournit le callback d'ouverture, pas l'instance unique sous Windows/Linux) |
| Menus natifs | NSMenu + menu du Dock sur macOS ; sous Windows et Linux, GPUI stocke les menus et l'app les dessine |
| Menus contextuels natifs | aucun backend n'en fournit : menus dessinés dans l'app |
| Vibrancy `hudWindow` | NSVisualEffectView (macOS), Mica/Acrylic (Windows), protocole de flou KDE seulement (Wayland) |
| Glisser-déposer depuis l'OS | oui sur les 4 backends desktop |
| `fontdb` | `all_font_names` du text system GPUI |
| `notify` 7 + FSEvents | `notify` (7 stable, 9.0 en release candidate) |
| Télémétrie PostHog | retirée en v1 (aucun besoin courant) |
| Variables CSS de thème | struct `Theme` dérivée de 5 valeurs par mode (accent, fond, texte, opacité, contraste) |

## 5. Inventaire de portage

| Domaine | Source Writer | Lignes | Réutilisation | Taille |
|---|---|---|---|---|
| Réglages | `config.rs`, `commands/settings.rs`, schéma | ~1 360 | telle quelle | S |
| Ignore, index, recherche | `ignore.rs`, `commands/search.rs` | ~710 | telle quelle, unifier les deux modèles d'ignore | S |
| FS et workspace | `commands/fs.rs`, `commands/workspace.rs` | ~2 525 | logique telle quelle, enveloppes à refaire | M |
| Watcher | `watcher.rs` | ~1 065 | logique telle quelle, émission à refaire | S-M |
| État multi-fenêtres | `state.rs` | ~670 | telle quelle sauf les labels de fenêtre | S |
| Récents, sessions, cibles, CLI | `recents.rs`, `open_target.rs`, `writer_cli.rs` | ~890 | telle quelle | S |
| Images, polices, démarrage | `images.rs`, `fonts.rs`, `startup.rs` | ~380 | telle quelle | S |
| Updater, dock, CLI install, `lib.rs` | | ~1 680 | réécrits par OS | M |
| Télémétrie | `telemetry.rs`, dialogue de consentement | ~1 090 | abandonnée en v1 | |
| Moteur d'édition | `prosemark-core` (~4 480), recherche, formatage, presse-papiers, menus, gel du drag | ~6 500 TS | réécrit | XL |
| Blocs riches | tables, images, wiki-links, HTML, math, Mermaid, frontmatter, section rail, titres | ~3 300 TS + 500 CSS | réécrits + moteurs de rendu | XL |
| Shell | stores, hooks, onglets, sidebar, palette, réglages, compact, accueil | ~10 000 TS | réécrit | L |
| Thème | `App.css`, `prosemark-theme.css`, presets | ~800 CSS | réécrit en tokens | S |
| Tests | 716 tests TS, 168 tests Rust | ~8 600 | invariants portés en Rust | M |

## 6. Phases

Chaque phase a un critère de sortie vérifiable. Total estimé : 26 à 41 semaines ; MVP à la fin de la phase 3 (16 à 25 semaines).

### Phase 0 : fondations et spikes (2 à 3 semaines)
- Une ADR par décision : moteur d'édition, parser, buffer (règle du §3.2), Mermaid, math. Envoyer la demande d'exception App Store à Joel.
- Workspace Cargo, GPUI épinglé selon le workflow de Paneflow, CI sur les 3 OS (build + tests).
- **Spike éditeur** (option C) : afficher une note de 1 Mo avec des titres de tailles variables, masquer `**`, `#` et URLs selon la sélection, un widget bloc (image), sélection au glisser, IME : composition CJK et touches mortes AZERTY sur macOS, Windows (IMM32 : MS-IME japonais, Pinyin), Wayland (text-input-v3), X11 (XIM).
- **Spike parser** : tree-sitter-md contre pulldown-cmark sur un corpus de fixtures.
- **Spike rendu** : `mermaid-svg` contre Pele dans `rquickjs` ; options math.
- **Sortie (go/no-go)** : frappe vers frame en moins de 16 ms au p99 sur 1 Mo, IME fonctionnel sur les 4 cibles desktop, critères parser du §3.5 tenus, ADR écrites.

### Phase 1 : cœur sans UI (2 à 3 semaines)
- Porter réglages, ignore, index, recherche, récents, sessions, cibles d'ouverture, CLI, watcher, opérations FS, images et polices dans `cerata_core`.
- Corriger en passant les défauts du §8 (n° 2 à 9 et 12).
- **Sortie** : les 168 tests Rust de Writer portés et verts sur les 3 OS, un test par défaut corrigé, benchmark d'indexation d'un workspace de 10 000 fichiers.

### Phase 2 : moteur d'édition (8 à 13 semaines)
- Buffer, transactions, undo/redo, multi-sélection (Cmd+D, Cmd+Shift+L), déplacement de lignes, Home/End visuels.
- Parser branché, arbre incrémental ; display map et height map à hauteurs variables.
- Règles hide/fold/reveal : titres, emphases, code inline, liens, autoliens, escapes, citations, fences, règles horizontales, tirets, emoji, images ; gel des décorations pendant un glisser ; flèches haut/bas qui entrent dans les blocs.
- Listes : Enter, Backspace, Tab/Shift-Tab, renumérotation, case à cocher au clic (pas au mousedown), géométrie de 3ch, retrait suspendu. Citations, fences (défilement horizontal par bloc, coloration tree-sitter), tabulation fixe de 4ch.
- Registre unique des ~22 commandes de formatage, qui alimente raccourcis et menus.
- Recherche et remplacement (littérale, insensible à la casse), marques sur la barre de défilement.
- Presse-papiers texte, collage d'image (5 Mo maximum), collage contenant un frontmatter.
- **Sortie** : invariants du §9 portés et verts ; snapshots de décorations conformes à l'oracle ; benchmark de frappe tenu.

### Phase 3 : application (4 à 6 semaines), MVP
- Fenêtres multiples à état propre avec epoch, instance unique, ouverture depuis l'OS (argv, associations, dépôt de fichiers).
- Onglets avec historique, launcher, Réglages en onglet, restauration de session (onglet actif d'abord), dernier Cmd+W qui masque la fenêtre.
- Sidebar : sections Épinglés/Récents/Tout, 6 tris, dossiers en premier, sélection multiple, renommage inline, déplacement par glisser, menus contextuels, duplication, suppression vers la corbeille avec confirmation si des fichiers ne sont pas sauvegardés.
- Palette (fichiers via l'index Rust, commandes), écran d'accueil, fenêtre compacte avec sélecteur de récents.
- Réglages : 43 clés, contrôle par type, portée globale ou workspace, écritures sérialisées par clé avec rollback.
- Thèmes : 5 valeurs vers tokens ; brancher enfin les 4 presets, jamais chargés dans Writer.
- Zoom (paliers fixes de 50 à 300 %), barre d'état, titre de fenêtre, barre de menus dessinée sous Windows et Linux.
- **Sortie** : invariants de `stores`, `settings-write-order`, `save`, `file-watcher`, `flatten-tree`, `tree-move` portés et verts ; usage quotidien sur les 3 OS pendant deux semaines.

### Phase 4 : blocs riches (5 à 8 semaines)
- Tables : rendu replié, markdown inline dans les cellules, colonnes dimensionnées entre largeur minimale et 48ch, source révélée au contact.
- Images locales, distantes et `![[embed]]` avec cache de hauteur ; wiki-links avec autocomplétion et navigation ; ancres de titres (slugs GFM) ; section rail ; panneau frontmatter.
- Blocs HTML : html5ever + ammonia avec la même liste blanche que DOMPurify, rendu d'un sous-ensemble.
- Math et Mermaid (pan, zoom, ajustement, plein écran, panneau d'édition).
- **Sortie** : corpus visuel de référence validé, invariants `table-decorations`, `math`, `mermaid`, `image-fold` portés.

### Phase 5 : intégration OS et distribution (3 à 4 semaines)
- Tout le §7.
- **Sortie** : installateurs signés pour les 3 OS, mise à jour automatique testée de N-1 vers N sur chacun.

### Phase 6 : durcissement (2 à 4 semaines)
- Performance : démarrage à froid, workspace de 50 000 fichiers, mémoire, taille du binaire.
- Accessibilité : brancher l'arbre AccessKit de l'éditeur (les 4 backends desktop le supportent).
- Matrice QA des distributions.
- **Sortie** : objectifs chiffrés atteints, fixés en phase 0 en mesurant Writer sur la même machine.

### Après la v1 : iOS
Adapter la mise en page, combler ou contourner les manques de `gpui_ios` (sélecteur de fichiers, presse-papiers d'images, accessibilité), et trancher la distribution selon la réponse de Joel (§3.2).

## 7. Matrice plateformes

| Fonction | macOS | Windows | Linux |
|---|---|---|---|
| Rendu GPUI | Metal | DirectX | wgpu, features `wayland` et `x11` |
| IME | NSTextInputClient | IMM32 (pas de TSF) | text-input-v3 (Wayland), XIM (X11) |
| Ouverture par l'OS | types de documents dans Info.plist, `on_open_urls` | associations par l'installateur, argv + instance unique | `.desktop` avec `MimeType=`, `xdg-mime`, argv + instance unique |
| Instance unique | réouverture par le Dock | named pipe | socket Unix ou D-Bus |
| Récents hors app | menu du Dock (GPUI) | Jump List (GPUI) | actions `.desktop` (statiques) |
| Barre de menus | native | dessinée par l'app | dessinée par l'app |
| Commande CLI | lien dans `~/.local/bin` ou `/usr/local/bin` | PATH utilisateur (`HKCU\Environment`) | `~/.local/bin` |
| Révéler un fichier | `open -R` | `explorer /select,` | D-Bus `FileManager1.ShowItems` |
| Terminal | `open -a <app>` | Windows Terminal, sinon cmd | `$TERMINAL`, `x-terminal-emulator`, réglage |
| Fond translucide | NSVisualEffectView | Mica/Acrylic | KDE seulement sous Wayland |
| Paquets | `.dmg` universel arm64 + x86_64, Developer ID + notarisation | MSI/NSIS ou MSIX, signé | AppImage, `.deb`, `.rpm`, Flatpak (Flathub), AUR |
| Mise à jour | Velopack ou updater | Velopack | Velopack (AppImage), Flathub, dépôts |

Pièges propres à chaque OS :

- **Raccourcis.** Cmd devient Ctrl sous Windows et Linux. Sous Windows, Ctrl+Alt équivaut à AltGr : les raccourcis de titres Cmd+Alt+0 à 6 entreraient en conflit avec la saisie de caractères en AZERTY (AltGr+0 donne `@`, AltGr+3 donne `#`). Prévoir un autre mapping.
- **Polices.** Writer utilise SF Pro et SF Mono par défaut : absentes hors macOS et réservées aux plateformes Apple par leur licence. Choisir des défauts par OS ou embarquer des polices libres.
- **Windows.** IME en IMM32 seulement : à tester tôt avec les IME japonais et chinois, contribution TSF upstream possible. Fins de ligne CRLF (CodeMirror normalise en LF : décider de préserver le style du fichier), rename atomique qui échoue si un antivirus ou l'indexeur tient le fichier (prévoir des réessais), chemins longs, système de fichiers insensible à la casse, caractères interdits `<>:"\|?*` au renommage.
- **Linux.** Limite inotify `max_user_watches` sur les gros workspaces ; bac à sable Flatpak : accès aux dossiers arbitraires et surveillance de fichiers à travers les portails, à tester tôt ; flou de fond absent hors KDE.
- **macOS.** Writer ne publie que de l'arm64 ; viser un binaire universel. Feature `font-kit` de GPUI obligatoire, sinon aucun glyphe n'est dessiné.

## 8. Défauts de Writer à ne pas reproduire

Vérifiés par moi : n° 1, 2, 5, 10, 14. Les autres viennent du rapport backend, avec références.

1. **Un changement externe écrase silencieusement les modifications non sauvegardées** : `use-file-watcher.ts:31-50` recharge depuis le disque sans regarder l'état « dirty » (`editor-store.ts:1183`). Prévoir un conflit explicite (garder, recharger, comparer).
2. Filtres d'extension incohérents : `.md` sensible à la casse dans l'index, la sidebar et le watcher (`fs.rs:174,294`, `search.rs:194`, `watcher.rs:506,597`), `.markdown` accepté seulement à l'ouverture (`open_target.rs:93`). Un seul prédicat.
3. Deux modèles d'ignore (`WalkBuilder` pour l'index, `WorkspaceIgnore` pour la sidebar) : la sidebar et la recherche divergent.
4. `index_workspace` synchrone (`search.rs:28`).
5. Séparateurs Windows : `rfind('/')` (`search.rs:112`), suppression du seul `/` final (`workspace.rs:892`).
6. Sessions et récents écrits sans verrou ni atomicité ; `set_workspace` écrase les éditions externes de la config ; ordre des nouvelles clés non déterministe.
7. Fenêtre de 2 s après une sauvegarde qui avale aussi de vrais changements externes, chemins non canonisés (`fs.rs:377`).
8. Sauvegarde temp + rename qui remplace les liens symboliques, perd permissions et attributs étendus, sans fsync.
9. Commandes FS sans confinement de chemin, scope d'assets `"**"`.
10. Détection du mode CLI par `argv[0]` insensible à la casse (`main.rs:21-29`) : « Writer » vaut « writer ». Impact réel non vérifié. Utiliser un binaire ou une sous-commande explicite.
11. L'instance unique ignore le répertoire courant du second lancement (`lib.rs:495`), et seule la première URL ouverte est traitée.
12. Titres lus de façon synchrone dans `read_directory` : bloque sur iCloud ou un montage réseau (spec slow-storage non implémentée).
13. Regex de frontmatter de `lib/frontmatter.ts:3` plus stricte que le parser de l'éditeur.
14. Presets de thème présents mais jamais chargés.
15. Renommage sans validation des caractères interdits.
16. Filtre de transaction des listes sans `sequential: true` (`list/index.ts:522-548`) : `RangeError` possible (inféré).

## 9. Stratégie de vérification

- **Invariants portés.** Les 716 tests TS et 168 tests Rust deviennent des tests Rust sur `cerata_markdown` et `cerata_core`. Suites prioritaires : `list-extension` (Enter, Backspace, Tab, rendu), `editor-formatting`, `setext-dash-underline`, `blockquote-enter`, `math`, `table-decorations`, `image-fold`, `drag-selection-gate`, `code-fence-scroll`, `wiki-links`, `paths`, `frontmatter`, `yaml-entries`, `heading-slug`, `document-stats`, `stores`, `save`, `settings-write-order`, `file-watcher`, `flatten-tree`, `tree-move`, `zoom`.
- **Oracle différentiel.** Le playground de Writer (`apps/playground`, éditeur exposé sur `window.__view`) génère, pour chaque fixture et position de curseur, les plages masquées et décorations calculées par CodeMirror. On compare la sortie de `decorate()` à ces snapshots.
- **Tests d'interaction** avec les helpers de GPUI : `TestAppContext` (`simulate_keystrokes`, `simulate_input`), `VisualTestContext` (clavier, souris, clic, `draw`), captures d'écran headless sur macOS et Linux seulement (`gpui_platform.rs:102-121`).
- **Corpus visuel** de référence pour tables, math et Mermaid.
- **Benchmarks** : reparse, `decorate`, layout, indexation, recherche ; démarrage mesuré au niveau de l'app.
- **CI** : build + tests unitaires sur macOS (arm64 et x86_64), Windows x64, Ubuntu. QA manuelle sur Ubuntu LTS, Debian, Fedora et Arch, sous GNOME et KDE, en Wayland et X11.

## 10. Risques

| Risque | Probabilité | Impact | Parade |
|---|---|---|---|
| Display map à hauteurs variables plus coûteuse que prévu (défilement, hit-testing, IME) | élevée | critique | spike go/no-go en phase 0, height map en `sum_tree` |
| Ruptures d'API GPUI | certaine | moyen | rev exact, montées groupées, workflow Paneflow |
| IME Windows limité à IMM32 | moyenne | élevé | test dès la phase 0, contribution TSF upstream |
| Rendu Mermaid ou math insuffisant | moyenne | moyen | phase 4 seulement, repli JS embarqué, puis bloc de code |
| Dérive de l'upstream Writer (425 commits en 6 mois) | certaine | moyen | base figée à `79b744c`, revue mensuelle du CHANGELOG |
| GPL et App Store iOS | moyenne | élevé | demande d'exception à Joel en phase 0, règle du §3.2 |
| `gpui_ios` jeune (2 semaines) et incomplet | élevée | moyen | iOS seulement après la v1 desktop |
| Binaire plus gros que Writer (~6 Mo) | élevée | faible | mesurer, ne pas en faire un argument |
| Coût d'opportunité face à Paneflow et Rust Doctor | moyenne | élevé | MVP en phase 3 et usage personnel immédiat |

## 11. Hors périmètre de la v1

Site web, télémétrie, tags, recherche plein texte (Cmd+Shift+F, spécifiée mais non construite dans Writer), MCP, snapshots de workspace, multi-fenêtre v2 (détacher un onglet), callouts et notes de bas de page, les améliorations personnelles après la parité, iOS et Android.

## 12. Points non vérifiés

- Qualité réelle de l'IME : IMM32 sous Windows, text-input-v3 et XIM sous Linux (le code existe, il n'a pas été testé).
- Chemin des associations de fichiers macOS vers `on_open_urls`, et alimentation de ce callback sous Windows.
- Granularité réelle des nœuds de tree-sitter-md par rapport à Lezer (spike parser).
- Pele et MathJax sous QuickJS sans DOM ; maintenance de `mitex`.
- Licence des icônes Hugeicons ; impact réel du défaut de détection CLI.
- Toutes les durées : ordres de grandeur, à recalibrer après la phase 0.
