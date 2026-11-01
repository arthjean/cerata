# ADR 0005 : rendu des diagrammes et des formules, Pele et MathJax dans un QuickJS isolé

> Statut : **décidé** le 2026-11-01. Story : US-031 (le PRD nomme ce fichier `0003-diagram-and-math-rendering.md`, mais le numéro 0003 est déjà pris par l'ADR du buffer). Contrainte : PRD, « Aucun moteur JavaScript appliqué au contenu d'un document ; seul un moteur de rendu de diagrammes ou de formules isolé, sans accès réseau ni fichiers, est permis si l'ADR de US-031 le retient ».

## Contexte

Writer rend Mermaid avec Pele (`mermaid-renderer.ts`, `pele` ^1.6.0) et les formules avec KaTeX (`math-renderer.ts`), les deux dans son webview. Cerata n'a pas de navigateur. Il faut un moteur qui produise du SVG hors du thread UI, que GPUI rastérise ensuite (`SvgRenderer` de GPUI : resvg avec les polices système chargées, texte converti en chemins).

## Mesures

Spike : `cargo run -p cerata_render --example render_spike --features spike --release`, sur la machine de référence Windows (`docs/benchmarks.md`), build release. Le dossier pointé par `CERATA_SPIKE_JS` contient les paquets npm `pele@1.13.0` et `mathjax@3.2.2`, et les paquets Typst `mitex:0.2.4` et `xarrow:0.2.0`, décompressés. Corpus (`examples/render_spike/corpus.rs`) : un diagramme par type de l'union `DiagramType` de Pele 1.13.0 (33 types, dont les 22 échantillons des tests de Writer) et 100 formules LaTeX courantes. Un élément compte comme rendu s'il sort sans erreur et que sa rastérisation par resvg n'est pas blanche. Le temps est le p95 de 20 rendus à froid par élément (cache de Typst vidé avant chaque rendu). L'écart visuel est la différence absolue moyenne des niveaux de gris, après mise à l'échelle des deux rendus dans un carré de 128 px, calculée sur les éléments rendus par les deux moteurs. Le poids est la taille du binaire release (LTO, strip) des exemples `weight_*` moins celle de `weight_base` (0,12 Mo).

| Candidat | Rendus | p95 | Écart visuel | Poids ajouté | Initialisation |
|---|---|---|---|---|---|
| Pele 1.13.0 dans `rquickjs` 0.14.0 | **33/33 (100 %)** | 0,76 ms | référence (moteur de Writer) | +1,79 Mo | 55 ms |
| `mermaid-svg` 0.7.0 | 22/33 (66,7 %) | 0,02 ms | 5,8 % vs Pele | +1,33 Mo | 0 |
| MathJax 3.2.2 (`tex-svg-full`, DOM lite) dans `rquickjs` | **100/100 (100 %)** | 6,33 ms | 5,5 % vs mitex + Typst | +3,35 Mo | 116 ms |
| `mitex` 0.2.4 + `typst` 0.15.1 + `typst-svg` | 96/100 (96 %) | 5,17 ms | 5,5 % vs MathJax | +36,4 Mo | 13 ms |
| Pele et MathJax dans le même binaire | | | | +4,06 Mo | |

Détail des échecs :

- `mermaid-svg` ne connaît pas `info`, `agentflow`, `usecase`, `railroad`, `treeView`, `venn`, `cynefin`, `wardley`, `swimlane`, `ishikawa` ni `eventmodeling`.
- mitex + Typst :
  - Échouent sur `\Delta`, `\partial`, `\setminus` et `\leftrightarrow`. Les symboles `diff` et `sect`, ainsi que des modificateurs de symbole, ont été renommés dans Typst 0.14 et 0.15, alors que le convertisseur `mitex` publié sur crates.io date de 2024.
  - La spec Typst de mitex a dû être corrigée d'une ligne (`sym.kai`) pour compiler.
  - `x^` passe sans erreur.
- MathJax rejette les trois formules invalides testées (`\frac{a`, `\undefinedmacro x`, `x^`) avec un message lisible (« Missing close brace », « Undefined control sequence \undefinedmacro »), une fois `noundefined` retiré et `formatError` configuré pour lever une exception.
- Pele rejette un diagramme invalide avec la ligne et la colonne.

Les planches `docs/captures/ep-005/us-031-diagrammes.png` (Pele à gauche, `mermaid-svg` à droite) et `us-031-formules-1.png`, `us-031-formules-2.png` (MathJax à gauche, mitex + Typst à droite) montrent chaque rendu. L'écart avec KaTeX, le moteur de Writer, n'est pas mesurable sans navigateur. Il est jugé sur la planche : MathJax et KaTeX implémentent le même modèle de mise en page TeX, avec les mêmes polices Computer Modern.

## Décision

**Diagrammes : Pele. Formules : MathJax 3.2.2 (`tex-svg-full` et l'adaptateur `liteDOM`).** Les deux tournent dans un même runtime QuickJS (`rquickjs`, sans feature), sur un thread dédié du crate `cerata_render`. Les deux options retenues dépassent 90 % de couverture, donc aucun repli n'est imposé par cette décision. Les stories US-036 et US-037 affichent néanmoins la source et l'erreur quand un rendu échoue.

Isolation (critère 3 de US-031, vérifiée par le spike) :

- **Ce que le contexte contient.** Le contexte est `Context::full`, c'est-à-dire les seules intrinsèques ECMAScript. rquickjs n'embarque ni le module `std`/`os` de quickjs-libc, ni `fetch`, ni `require`, et aucun chargeur de modules n'est installé. Le spike vérifie que `fetch`, `XMLHttpRequest`, `require`, `process`, `std`, `os`, `Deno`, `Bun`, `importScripts` et `WebSocket` y valent tous `undefined`.
- **Ce que le moteur reçoit.** La seule entrée est une chaîne, la variable globale `__src`. La seule sortie est une chaîne SVG.
- **Le cas de MathJax.** Le moteur vocal SRE de `tex-svg-full` réclame un `require` Node au chargement : des bouchons inertes (`process`, `require`) sont définis le temps du chargement, puis supprimés.
- **Les limites.** Le runtime a une limite mémoire de 256 Mo et une pile de 4 Mo. Un gestionnaire d'interruption abandonne un rendu qui dépasse son échéance (500 ms, critère de US-036).

Le SVG produit passe par GPUI (`img` sur une `Image` au format SVG), qui ne charge aucune ressource externe. Les couleurs de Pele sont des propriétés CSS personnalisées que resvg ne résout pas : `cerata_render` y substitue les couleurs du thème avant de remettre le SVG à GPUI.

## Conséquences

- `cerata_render` embarque deux bundles, versionnés dans `crates/cerata_render/assets/js/` (Pele, MIT ; MathJax, Apache-2.0, compatible avec la GPLv3), et la mention de leurs licences dans `NOTICE`. Une mise à jour remplace le fichier et relance le spike.
- Pele est le moteur de Writer : un diagramme se rend comme dans Writer, aux polices près.
- Le premier diagramme ou la première formule paie l'initialisation de son moteur (55 ms et 116 ms), une seule fois et hors du thread UI.
- `mitex` + Typst reste écarté tant que le convertisseur ne suit pas Typst : le binaire grossirait de 36 Mo pour une couverture moindre.

## Non prouvé

- Le rendu des 11 types que `mermaid-svg` ignore n'est vérifié que par le spike : Writer ne les teste pas.
- La couverture mesurée sur un corpus de 100 formules ne prédit pas celle de toutes les notes d'un utilisateur.
- Les temps de rendu sous macOS et Linux relèvent de US-045 et US-046.
