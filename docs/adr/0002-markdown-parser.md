# ADR 0002 : parser Markdown, pulldown-cmark avec reconstruction des marqueurs

> Statut : **décidé**, mesuré sous Windows le 2026-10-31. pulldown-cmark 0.13.6 est retenu ; tree-sitter-md 0.5.4, recommandé par `PLAN.md` §3.5, est écarté sur mesures.
> Story : US-003. Plan : `PLAN.md` §3.4 (point 2, `MarkdownTree`), §3.5 (parser), §10 (risque 4).
> Référence fonctionnelle : writer-computer au commit `79b744c`. Versions : `tree-sitter` 0.27.1, `tree-sitter-md` 0.5.4 (feature `parser`), `pulldown-cmark` 0.13.6.

## Contexte

Le live preview de Writer lit un arbre Lezer enrichi de 8 règles personnalisées (`prosemark-core/markdown/index.ts:24-34`) et d'une regex de liens wiki. Toutes les règles de décoration de Cerata (US-011 et suivantes) liront le `MarkdownTree` de US-010 : changer de parser après coup obligerait à réécrire cette couche. Le spike compare les deux candidats du §3.5 sur trois questions : quelles règles chaque parser couvre nativement, s'il expose les marqueurs (délimiteurs d'emphase, marqueurs de titre et de liste, destinations de lien), et s'il tient le budget de reparse après une frappe (moins de 2 ms sur la liste de 3 000 items, moins de 16 ms sur 1 Mo).

## Ce qui a été construit

Un exemple cargo jetable du crate `cerata_markdown`, sans toucher `src/` :

```
cargo run --release -p cerata_markdown --example parser_spike -- all
cargo run --release -p cerata_markdown --example parser_spike -- trees <fixture>..
cargo test -p cerata_markdown --example parser_spike
```

| Fichier (`crates/cerata_markdown/`) | Rôle |
|---|---|
| `examples/parser_spike.rs` | CLI (`trees`, `coverage`, `bench`, `all`) et 5 tests qui figent la matrice de couverture, l'inventaire des marqueurs, le défaut EOF et la justesse de l'incrémental |
| `examples/parser_spike/ts.rs` | orchestration à deux niveaux de tree-sitter-md (arbre de blocs incrémental, un arbre inline par nœud `inline`, en coordonnées relatives, reparsé seulement s'il touche l'édition ou les `changed_ranges`), plus le `MarkdownParser` du crate comme référence naïve |
| `examples/parser_spike/pd.rs` | pulldown-cmark : événements avec plages (`into_offset_iter`) |
| `examples/parser_spike/probes.rs` | extraction, dans la sortie native de chaque parser, de ce que les tests de Writer observent |
| `examples/parser_spike/corpus.rs` | 54 cas avec le résultat attendu de Writer, générateurs de la liste de 3 000 items et de la note de 1 Mo |
| `examples/parser_spike/bench.rs` | latence de reparse |
| `tests/fixtures/parser_spike/*.md` | 54 fixtures octet pour octet, plus `markers.md` ; un `.gitattributes` local les garde en LF malgré `core.autocrlf=true` |

`Cargo.toml` du crate : la dépendance de dev `tree-sitter-md` active la feature `parser` (pour mesurer `MarkdownParser`), et une entrée `[[example]] test = true` fait tourner les tests du spike dans `cargo test`. Aucune autre dépendance.

## Corpus

Au moins un cas par règle, plus tous les cas des trois fichiers de tests cités par la story : `tests/math.test.ts` (8 cas de parsing), `tests/markdown-links.test.ts` (4), `tests/setext-dash-underline.test.ts:22-37` (7, chacun en deux variantes : sans et avec saut de ligne final, car la dernière ligne d'un buffer n'en a souvent pas). Les cas ajoutés visent les bords des règles : frontmatter hors offset 0, après une ligne vide, fermé en fin de fichier, non fermé, vide ; `$` dans un code span ; `I paid $5 and $x$` ; `----` (aucun tiret rendu) ; `10:30` et `1:2:3` pour l'emoji (Writer produit `:2:`) ; `![[img.png]]` et `[[a|b]]`. Les résultats attendus sont dérivés du code Lezer cité sur chaque règle ; Writer lui-même n'a pas été exécuté (pas de `node_modules` dans le dépôt de référence).

## Couverture native

Nombre de cas où la sortie native égale celle de Writer. tree-sitter est lu avec un saut de ligne virtuel ajouté après le buffer (voir les défauts plus bas) ; la colonne « brut » donne le résultat sans ce contournement.

| Règle | Cas | tree-sitter brut | tree-sitter | pulldown-cmark |
|---|---:|---:|---:|---:|
| Frontmatter à l'offset 0 | 6 | 5 | 6 | 3 |
| Math Pandoc `$…$` et `$$…$$` | 11 | 7 | 7 | 10 |
| Destinations de lien avec espaces | 4 | 1 | 1 | 1 |
| Liens imbriqués en texte | 2 | 0 | 0 | 1 |
| Setext : un seul `-` n'est pas un H2 | 14 | 5 | 8 | 8 |
| Escape de tout caractère | 3 | 1 | 1 | 0 |
| Tirets `--` et `---` | 5 | 1 | 1 | 5 |
| Emoji `:mot:` | 5 | 1 | 1 | 1 |
| Liens wiki | 4 | 0 | 0 | 4 |

Aucune règle n'est entièrement native chez tree-sitter hors frontmatter ; chez pulldown-cmark, tirets et liens wiki le sont, la math l'est à un cas près.

### Comment chaque règle s'obtient

| Règle | tree-sitter-md 0.5.4 | pulldown-cmark 0.13.6 |
|---|---|---|
| Frontmatter | **natif** : `minus_metadata` n'est valide qu'en tête de document (grammaire), y compris `---\n---` | **post-traitement** : `MetadataBlock` est accepté après un paragraphe ou une ligne vide, et refusé vide (première ligne vide interdite, `scanners.rs:752-782`). Rejeter un bloc hors offset 0 impose de reparser sa plage sans l'option |
| Math | `EXTENSION_LATEX` est **déjà compilée** dans le crate (`latex_block` et `latex_span_delimiter` figurent dans `node-types.json`), mais avec la sémantique des code spans : `$5 and $`, `$ b$` et `$b $` deviennent des formules. Exact : **modifier le scanner C** (`LATEX_SPAN_START`/`LATEX_SPAN_CLOSE`, scanner inline) ; un post-traitement n'est qu'approché, car l'intérieur d'une fausse formule n'a pas été analysé en inline | **natif** (`ENABLE_MATH`, garde-fous Pandoc) ; seul écart : `I paid $5 and $x$` donne `$x$` là où Writer donne `$5 and $x$`. Post-traitement ou écart accepté |
| Liens avec espaces | **post-traitement** (un `shortcut_link` suivi de `(…)` sur la même ligne) ou modification de `grammar.js` puis régénération ; pas de scanner | **post-traitement** : le lien sort en `Text` `[`, `…`, `]`, `(…)` |
| Liens imbriqués | **post-traitement** ; de plus `[a [b] c](d)` ne produit aucun lien (le `[b]` devient un `shortcut_link` qui casse le lien externe, alors que CommonMark le garde) | **post-traitement** pour `[a [b](c) d](e)` (CommonMark fait gagner le lien interne) ; `[a [b] c](d)` est natif |
| Setext à un `-` | **modifier le scanner C** (`parse_minus`, condition `minus_count >= 1` du soulignement, scanner de blocs) ou post-traitement (réécrire `setext_heading` en paragraphe plus liste vide) | **post-traitement**, même réécriture d'un `Heading` H2 dont le soulignement est un seul `-` |
| Escape étendu | natif pour la ponctuation ASCII (`backslash_escape`) ; **post-traitement** pour le reste (le `\` reste un jeton anonyme) | **post-traitement** : aucun événement d'escape, le `\` est simplement absent des plages `Text` ; tous les escapes se reconstruisent par les trous |
| Tirets | **post-traitement** sur les jetons `-` anonymes | **natif** avec `ENABLE_SMART_PUNCTUATION` : un `Text` tiret demi-cadratin ou cadratin dont la plage source est `--` ou `---`, `----` rejeté comme Writer. Cette option réécrit aussi guillemets et points de suspension, mais seulement dans le contenu des `Text`, que l'éditeur n'affiche pas : les plages restent exactes |
| Emoji | **post-traitement** sur les jetons `:` | **post-traitement** sur le texte |
| Liens wiki | **régénération** avec `EXTENSION_WIKI_LINK` : non compilée dans le crate publié (aucun `wiki_link` dans `node-types.json`), et le crate ne livre pas `common/grammar.js`, donc il faut le dépôt amont et le CLI tree-sitter. Ou post-traitement par la regex de Writer, qui doit alors écraser le `shortcut_link` que tree-sitter voit dans `[[x]]` | **natif** (`ENABLE_WIKILINKS`) : `[[x]]`, `[[a\|b]]`, `![[img.png]]`, ignoré dans le code |

### Marqueurs

tree-sitter-md expose bien les marqueurs, vérifié sur `markers.md` et figé par un test : `atx_h1_marker`, `setext_h1_underline`, `emphasis_delimiter` (un nœud par caractère : `**` donne deux nœuds, `~~` deux `strikethrough` imbriqués), `code_span_delimiter`, `link_text`, `link_destination`, `link_title`, crochets et parenthèses en nœuds anonymes, `image_description`, `uri_autolink`, `backslash_escape`, `latex_span_delimiter`, `list_marker_minus`, `list_marker_dot`, `task_list_marker_checked`, `block_quote_marker`, `fenced_code_block_delimiter`, `info_string`, `pipe_table_delimiter_row`. L'hypothèse du §3.5 sur la granularité est confirmée.

pulldown-cmark n'en expose aucun, hors `TaskListMarker`. Chaque événement porte la plage de sa construction entière (`Start(Strong)` couvre `**bold**`, le `Text` intérieur couvre `bold`), les marqueurs sont donc les différences de plages ; la destination et le titre d'un lien ne sont donnés qu'en valeur (`dest_url`, `title`), sans plage.

## Défauts de tree-sitter-md constatés

- **EOF n'est pas une fin de ligne** pour le scanner de blocs : `Title\n--`, `Title\n=` et un frontmatter fermé sur la dernière ligne sans `\n` ne sont pas reconnus, et `text\n- ` en fin de buffer produit un nœud `ERROR`. Contourné dans le spike en présentant au parser un `\n` virtuel après le buffer (aucun effet sur les plages réelles).
- `[a [b] c](d)` ne donne aucun lien (voir plus haut).
- Le README du crate prévient : « it is not recommended to use this parser where correctness is important. The main goal for this parser is to provide syntactical information for syntax highlighting ».
- `TreeCursor::goto_first_child_for_byte` renvoie `None` quand l'octet tombe entre deux enfants (une ligne vide) au lieu de l'enfant suivant ; le test d'édition aléatoire du spike l'a révélé (arbres inline perdus), d'où un repli linéaire.
- `MarkdownParser`, le wrapper du crate, édite tous les arbres inline et rappelle le parser inline sur chaque nœud à chaque reparse : inutilisable tel quel (mesures plus bas). Cerata devrait posséder l'orchestration à deux niveaux.

## Mesures

Machine : AMD Ryzen 7 7800X3D (8 cœurs, 16 threads), 31 Go, Windows 11 Professionnel 10.0.26300, rustc 1.98, profil `release` du workspace (`lto = true`, `codegen-units = 1`). D'autres agents compilaient en parallèle : charge CPU de 25 % pendant le run retenu ci-dessous, 38 à 62 % pendant trois runs antérieurs.

Méthode (`bench.rs`) : chaque itération insère un caractère au point d'édition, reparse (chronométré), le supprime, reparse (chronométré) ; 500 itérations après 10 de chauffe, soit 1 000 échantillons par ligne (80 pour `MarkdownParser`, trop lent). Deux éditions : `x` et un saut de ligne, qui change la structure de blocs. Le travail du buffer (insertion dans la `String`, calcul ligne et colonne, qu'une rope fait en O(log n)) reste hors chrono. Côté tree-sitter, le chrono couvre `Tree::edit`, le reparse incrémental de l'arbre de blocs, `changed_ranges`, la recherche des nœuds inline touchés et leur reparse. Après la boucle, l'arbre incrémental est comparé nœud par nœud (type et plage absolue) à un parse neuf, après une insertion et après une suppression : identiques à chaque fois ; le test `incremental_reparse_equals_fresh_parse` fait la même vérification sur 400 éditions aléatoires qui changent la structure. Côté pulldown-cmark, le chrono couvre un reparse complet qui range chaque événement et sa plage dans un tampon réutilisé : c'est un plancher, la construction de l'arbre et des marqueurs n'y est pas.

Liste de 3 000 items : la note de `e2e/specs/list-enter-exit.spec.js:238-251` reproduite à l'identique (381 891 octets, 3 004 lignes), édition juste après `sub item!!` comme le fait le spec. Note de 1 Mo : générée, 1 048 858 octets, 52 069 lignes, titres H1 à H6, emphases, code, liens, math, emoji, tirets, liens wiki, listes et tâches, citations, fences, tableaux, blocs `$$`, setext ; édition au milieu d'un paragraphe.

| Note, édition | tree-sitter incrémental p50 / p99 | dont arbre de blocs p50 / p99 | `MarkdownParser` p50 / p99 | pulldown-cmark complet p50 / p99 |
|---|---|---|---|---|
| Liste 3 000, `x` | 4,337 / 23,839 ms | 3,300 / 21,072 ms | 108,982 / 145,616 ms | **0,732 / 0,927 ms** |
| Liste 3 000, saut de ligne | 4,288 / 23,795 ms | 3,257 / 21,233 ms | 112,203 / 144,971 ms | **0,804 / 1,348 ms** |
| 1 Mo, `x` | **1,451 / 7,152 ms** | 0,735 / 6,194 ms | 865,058 / 1 111,733 ms | 7,890 / 9,510 ms |
| 1 Mo, saut de ligne | **1,396 / 6,833 ms** | 0,714 / 5,896 ms | 828,168 / 890,054 ms | 7,878 / 9,446 ms |

Parse initial (médiane de 5) : liste, tree-sitter 208 ms (arbre de blocs seul 93 ms), pulldown-cmark 0,77 ms ; 1 Mo, tree-sitter 1 188 ms (blocs seuls 357 ms), pulldown-cmark 8,5 ms. tree-sitter-md analyse environ 1 Mo/s, pulldown-cmark environ 120 Mo/s.

Sous charge (runs antérieurs, 38 à 62 %) : pulldown-cmark reste sous 2 ms au p99 sur la liste (1,08 à 1,66 ms) ; sur 1 Mo son p99 va de 9,4 à 24,8 ms (deux mesures sur six au-dessus de 16 ms) ; tree-sitter monte jusqu'à 69 ms au p99 sur la liste.

Lecture : sur la liste, le reparse incrémental de **l'arbre de blocs seul** de tree-sitter-md coûte déjà 3,3 ms au p50, au-dessus du budget de 2 ms, quelle que soit l'orchestration des arbres inline (un seul arbre inline est reparsé par édition). Aucun travail sur les règles ne corrige cela. pulldown-cmark tient les deux budgets au p99 sur une machine peu chargée.

## Règle de décision de la story

Règles qui exigent de modifier le scanner C de tree-sitter-md pour un résultat exact : **2** (math Pandoc, scanner inline ; setext à un `-`, scanner de blocs), 3 si l'on compte le défaut EOF, contournable sans patch. Le seuil « plus de trois » n'est pas atteint : la règle ne force pas pulldown-cmark. La décision vient du critère de performance, qui doit être tenu « pour le parser retenu », et de la justesse.

## Décision

**pulldown-cmark 0.13.6**, options `ENABLE_TABLES | ENABLE_STRIKETHROUGH | ENABLE_TASKLISTS | ENABLE_MATH | ENABLE_YAML_STYLE_METADATA_BLOCKS | ENABLE_WIKILINKS | ENABLE_GFM` (plus `ENABLE_SMART_PUNCTUATION` si l'on garde les tirets natifs), reparse complet à chaque édition, puis construction du `MarkdownTree` de US-010 et reconstruction des marqueurs depuis les plages.

Pourquoi :
1. C'est le seul candidat qui tient les deux budgets. tree-sitter-md manque celui de la liste dès le p50, et son parse initial (1,2 s pour 1 Mo) imposerait un parse en arrière-plan à l'ouverture.
2. Il couvre nativement plus de règles de Writer : math avec garde-fous Pandoc (10/11), tirets (5/5), liens wiki (4/4), là où tree-sitter demande un patch du scanner pour la math et une régénération de grammaire pour les liens wiki.
3. Sa conformité CommonMark est l'objectif du projet ; celle de tree-sitter-md ne l'est pas (README, défaut EOF, `[a [b] c](d)`).
4. Pur Rust, sans code C généré à vendoriser et à patcher.

Ce que l'on perd : l'incrémentalité et les nœuds de marqueurs natifs de tree-sitter. Le premier point est couvert par la vitesse brute ; le second se paie en code, chiffré ci-dessous.

## Coût de la reconstruction (estimation, non mesurée)

| Lot | Contenu | Taille estimée |
|---|---|---|
| Arbre | arène de nœuds (type, plage, parent, enfants, voisins) construite depuis `into_offset_iter`, API de US-010 | 250 à 300 lignes |
| Marqueurs | HeaderMark (`#` et séquence fermante, soulignement setext), EmphasisMark (plage du nœud moins celle des enfants), CodeMark (runs de backticks, lignes de fence, info string), LinkMark, URL et LinkTitle (relecture de la queue `](…)` avec la grammaire CommonMark des destinations et titres, formes par référence), QuoteMark (relecture des lignes, conteneurs imbriqués, continuations paresseuses), ListMark, FrontmatterMark, MathMark, Escape (trous entre plages `Text`), séparateurs de tableau et ligne d'alignement (absente des événements) | 600 à 800 lignes |
| Règles de Writer | frontmatter seulement à l'offset 0 et vide accepté, setext à un `-`, destinations avec espaces, liens imbriqués, escape étendu, emoji, écart `$5 and $x$` si on le corrige | 400 à 500 lignes |
| Tests | tests de Writer portés, corpus de ce spike réutilisé comme oracle, fuzz de US-010 | environ 400 lignes |

Total : 1 700 à 2 000 lignes, **7 à 10 jours-personne**. Les deux lots risqués sont la relecture des queues de liens et les marqueurs de citation dans des conteneurs imbriqués. Coût à l'exécution : linéaire en nombre d'événements (15 011 sur la liste, 177 346 sur 1 Mo) ; estimé à 20 à 50 % du parse, soit 10 à 12 ms au p50 sur 1 Mo, à mesurer dans US-010.

## Conséquences

- US-010 : « incrémental » se lit « recalculé à chaque édition, toujours complet ». Le critère de l'arbre jamais partiel est tenu par construction, puisque tout le document est reparsé.
- La marge sur 1 Mo est la moitié d'une frame (7,9 ms au p50, 9,5 ms au p99 à vide, jusqu'à 24,8 ms au p99 sous forte charge), avant la reconstruction. Si la latence frappe vers frame de US-002 dépasse 16 ms au p99 sur 1 Mo une fois l'arbre réel branché, deux replis, dans cet ordre : reparser seulement la fenêtre des blocs de premier niveau touchés (resynchronisation sur une frontière de bloc après l'édition, validée par un fuzz contre le parse complet ; une fence ou un `$$` non fermé étend la fenêtre jusqu'à la fin), puis parser en arrière-plan avec l'arbre précédent décalé par l'édition pour la frame courante.
- Un seul parseur de frontmatter : celui de US-010 enveloppe `MetadataBlock` avec la règle d'offset 0 de Writer, et sert aussi à la lecture des fichiers.
- pulldown-cmark (MIT) devient une dépendance normale de `cerata_markdown` ; `tree-sitter` et `tree-sitter-md` restent des dépendances de dev du spike, à retirer quand l'exemple sera supprimé.
- `PLAN.md` §3.5 et le risque 4 du PRD sont à mettre à jour : le repli devient le choix.

## Non prouvé

- Les résultats attendus de Writer sont lus dans son code, pas obtenus en exécutant Lezer.
- Le coût de la reconstruction des marqueurs et des post-traitements est estimé, pas mesuré.
- Mesures sur une seule machine, Windows, en partage avec d'autres builds ; rien sur macOS ni Linux.
- tree-sitter-md régénéré (wiki links) ou patché (math, setext) n'a pas été construit : son effet sur la vitesse est inconnu, mais l'arbre de blocs, qui suffit à manquer le budget, ne serait pas concerné par ces patchs inline, sauf le setext.

## Amendement US-010 (2026-11-01) : inline porté de Lezer, reparse fenêtré

Mise en œuvre dans `crates/cerata_markdown/src/tree/`. Deux corrections techniques par rapport à la décision ci-dessus, sans changer le choix de pulldown-cmark.

**Inline porté de `@lezer/markdown` 1.6.3.** pulldown-cmark ne fournit que la structure de blocs (événements et plages, option `ENABLE_TABLES` seule). L'inline est un portage direct du parser inline de Lezer (`InlineContext`, délimiteurs, résolution des emphases et des liens), plus Strikethrough et Autolink de GFM et les extensions inline de Writer, dans l'ordre des parsers de Writer (`tree/inline.rs`). Raison : reconstruire les marqueurs depuis les événements pulldown ne donnait pas l'arbre de Writer sur les cas limites (délimiteurs non appariés, liens imbriqués, escapes étendus) ; un portage ligne à ligne le donne par construction. Les noms et plages des nœuds sont ceux de Lezer. Preuve : `tests/tree_oracle.rs` compare l'arbre de Cerata à celui de Writer exécuté sous Bun (`scripts/oracle/oracle.ts trees`, prosemark-core au commit de référence) sur 92 fixtures (37 du corpus `tests/fixtures/corpus`, 54 du spike, plus `markers.md`) : 92/92 identiques. La section « Non prouvé » plus haut est donc levée pour l'arbre : Writer a été exécuté.

**Reparse fenêtré, premier repli des Conséquences.** Le parse complet avec l'arbre réel dépasse le budget : environ 2,7 ms sur la liste de 3 000 items et 22 ms sur 1 Mo (machine chargée). `MarkdownTree::reparse` (`tree/incremental.rs`) reparse seulement les blocs de premier niveau touchés plus un voisin de chaque côté, ou, dans une grande liste, les items touchés plus un voisin ; un bloc sentinelle de chaque côté doit ressortir identique, sinon tout le document est reparsé. Les éditions qui touchent un `$`, la première ligne ou le frontmatter, ou un document finissant par un bloc `$$` non fermé, reparsent tout. `EditorState` l'utilise à chaque transaction.

| Note, édition (`examples/tree_bench.rs`, 2 000 échantillons) | p50 | p99 | Budget |
|---|---|---|---|
| Liste 3 000, frappe après `sub item!!` | 0,201 ms | 0,294 ms | 2 ms |
| 1 Mo, frappe au milieu d'un paragraphe | 1,390 ms | 2,022 ms | 16 ms |

Justesse : `tests/tree_fuzz.rs` applique 10 000 éditions aléatoires (graine fixe, jetons de syntaxe et Unicode) et compare à chaque pas l'arbre fenêtré au parse complet ; plus de 5 000 passent par la fenêtre, toutes identiques, aucune panique.

**Écart assumé avec Writer.** Un `$$` en début de paragraphe de premier niveau, sans `$$` fermant plus loin, ouvre un nœud `Math` de bloc jusqu'à la fin du document, comme l'exige le critère de US-010. Writer laisse ce texte en paragraphe.
