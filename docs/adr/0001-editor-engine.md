# ADR 0001 : moteur d'édition, élément GPUI dédié à hauteurs de ligne variables

> Statut : **go pour EP-001** (option C), mesuré sous Windows le 2026-10-31. Les vérifications restantes ne relèvent plus de EP-001 : latence et IME sous Linux (Wayland et X11) suivis par US-045 (EP-007, validation Linux, testée par Arthur), latence et IME sous macOS suivis par US-046 (EP-008, validation macOS sur un Mac Scaleway). Pinyin sous Windows reste à vérifier à la main, par Arthur, en US-045 (voir IME).
> Story : US-002. Plan : `PLAN.md` §3.4 (option C), §7 (matrice plateformes), §10 (risques).
> GPUI : rev `37faef81f74fc1bb08be9939c4cffc8ede6ced2f`. Référence fonctionnelle : writer-computer au commit `79b744c`.

## Contexte

L'éditeur de Zed impose une hauteur de ligne unique, arrondit ses blocs à un multiple de cette hauteur et ne sait pas révéler la source au contact de la sélection (`PLAN.md` §3.4). Cerata doit donc écrire son propre élément GPUI : chaque ligne mise en forme à sa propre taille de police (titres H1 à H6 aux échelles 1.6/1.4/1.2/1.1/1/0.9 de Writer), une height map pour le défilement et le hit-testing, des marqueurs masqués hors sélection, des widgets bloc à leur hauteur exacte, et l'IME de chaque OS. C'est la décision la plus difficile à défaire du projet : si cet élément ne tient pas la latence ou ne peut pas porter l'IME, l'alternative est de garder Tauri (portage du webview) ou de revoir le périmètre du live preview.

Le spike répond à une question : un élément GPUI de ce type tient-il 16 ms au p99 sur une note de 1 Mo, et l'IME passe-t-il par `EntityInputHandler` sans contournement ?

## Ce qui a été construit

Un prototype jetable, en exemple cargo : `cargo run -p cerata --example editor_spike --release -- [<note.md>]`.

| Fichier (`crates/cerata/examples/`) | Rôle |
|---|---|
| `editor_spike.rs` | CLI, fenêtre 1200×800, raccourcis, placement du caret pour le benchmark |
| `editor_spike/buffer.rs` | buffer `String` avec index de lignes en octets et en UTF-16 |
| `editor_spike/height_map.rs` | height map, une entrée par ligne, sommes préfixes dans un arbre de Fenwick (`find(y)` en O(log n)) |
| `editor_spike/markup.rs` | scanner de ligne (`#`, `**`, liens, image seule sur sa ligne), décision pure `decorate_line(syntax, line_start, selections)`, métriques verticales de Writer |
| `editor_spike/display.rs` | correspondance offsets buffer et offsets affichés quand des plages sont masquées |
| `editor_spike/editor.rs` | entité éditeur : sélection, texte marqué, défilement ancré, `layout_line` à la demande, hit-testing, `EntityInputHandler` |
| `editor_spike/element.rs` | `Element` virtualisé : lignes visibles plus 300 px de marge, sélection, texte, widgets image (`AnyElement`), caret |
| `editor_spike/bench.rs` | `--bench-typing` |
| `editor_spike/fixture.rs` | générateur déterministe de la note de 1 Mo et de ses figures SVG |
| `editor_spike/tests.rs` | tests d'interaction GPUI (`VisualTestContext`) |
| `editor_spike/scripts/*.ps1` | sondes Windows : clavier et IME réels, glisser à la souris réelle |

Choix de conception validés par le spike, à reprendre dans le vrai moteur :

- **Une ligne, une mise en forme.** `WindowTextSystem::shape_text(texte affiché, taille de la ligne, runs, largeur de retour)` par ligne ; la graisse, la couleur et le soulignement varient par run. Le cache de mise en forme de GPUI (clé : texte, taille, runs, largeur) rend gratuites les lignes inchangées d'une frame à l'autre : une frappe ne remet en forme que la ligne éditée.
- **Métriques de Writer** (`prosemark-theme.css:155-187` et `:259-271`) : corps 16 px, interligne 1,875 (30 px) ; titre de niveau n : taille 16 × échelle, interligne 1,3 × taille, `padding-top = max(0, 12 − leading)`, `padding-bottom = max(0, 8 − 6 − leading)` avec `leading = (1,3 − 1,17) × taille / 2`.
- **Height map** à hauteurs estimées (½ em par caractère, comme les estimations de CodeMirror) puis exactes dès qu'une ligne est mise en forme. Le défilement est stocké comme (ligne d'ancrage, décalage dans la ligne) : corriger la hauteur d'une ligne au-dessus de l'ancre ne déplace rien à l'écran, ce qui supprime le problème des estimations décrit dans `docs/editor.md:127-160` de Writer.
- **Positions calculées depuis le modèle**, jamais depuis la frame précédente : `bounds_for_range`, `character_index_for_point` et le hit-testing remettent la ligne en forme à la demande (coup de cache en pratique). macOS demande la position du caret juste après `setMarkedText`, avant la frame suivante : un instantané de la dernière frame y serait périmé.
- **Masquage** : décision pure par ligne, règle de Writer (`hide/core.ts:35-124`, `rangeTouchesRange` inclusif aux deux bornes). Pendant un glisser, les décisions sont figées sur la sélection d'avant l'appui (équivalent de `unfurlFreezeFacet`) : le texte ne se réorganise pas sous le pointeur ; il se révèle au relâchement.
- **Image bloc** : la ligne `![alt](url)` est remplacée par le widget (sélection hors du nœud) ou montrée en source avec le widget dessous (sélection au contact), comme `fold/image.ts:80-120`. La taille naturelle est lue dans l'en-tête du fichier (SVG `width`/`height`, PNG IHDR) sans décoder l'image, ce qui joue le rôle du cache de hauteurs de Writer (`fold/image.ts:7-42`) : la hauteur est connue avant le décodage asynchrone, donc rien ne saute.

## Mesures

### Machine et méthode

Machine de référence Windows de `docs/benchmarks.md` : AMD Ryzen 7 7800X3D (8 cœurs, 16 threads), 32 Go, NVIDIA GeForce RTX 4070 Ti SUPER (pilote 32.0.16.2101, device Direct3D 11.1 d'après le journal de GPUI), écran 2560×1440 à **240 Hz**, facteur d'échelle 1, Windows 11 Pro 26H2 (10.0.26300.9818), mode d'alimentation « Performances optimales », rustc 1.98.0, build `--release` (LTO, `codegen-units = 1`). Écart à la règle de `docs/benchmarks.md` : d'autres applications étaient ouvertes (terminaux, autres agents de compilation).

Fixture : `fixture::generate(1 << 20)`, 1 049 777 octets, 14 442 lignes, 71 H1 et 701 titres de chaque niveau H2 à H6, 140 images bloc, 8 040 `**…**`, 7 927 liens. Commande :

```sh
cargo run -p cerata --example editor_spike --release -- --bench-typing
```

Le benchmark place le caret à la fin d'un paragraphe de plus de 200 caractères au milieu de la note (ligne ~7 220, titres, gras et liens visibles autour), puis tape 200 caractères (« the quick brown fox… », espaces compris, la ligne s'allonge et se replie) par `EntityInputHandler::replace_text_in_range`, le point d'entrée qu'appellent le clavier et l'IME de chaque plateforme. Une frappe par frame au plus : la suivante part après un délai pseudo-aléatoire uniforme dans [0 ; 16,7 ms) après la présentation de la précédente, donc à toutes les phases du rafraîchissement, comme une vraie frappe. Horodatages par frappe :

- *entrée* : juste avant `replace_text_in_range` ;
- *render* : `Render::render` de l'éditeur, début du travail de la frame ;
- *fin de paint* : fin du `paint` de l'élément ;
- *présentée* : tâche de premier plan créée à la fin du `paint`. Le callback de frame de GPUI dessine puis appelle `present` (DXGI `Present`) avant de rendre la main à la boucle de messages : la tâche s'exécute après le retour de `Present`. La latence du compositeur (DWM) et de l'écran n'est pas comprise.

Puis 60 sauts de défilement de 2,5 hauteurs de vue, chacun sur des lignes jamais mises en forme (frame entièrement à froid). Le rapport vérifie que le texte tapé est bien dans le buffer. Cinq exécutions après trois de chauffe ; le tableau donne la médiane des cinq, en ms.

### Résultats (Windows, 2026-10-31)

| Mesure (200 frappes, ou 60 sauts) | p50 | p95 | p99 | max |
|---|---|---|---|---|
| frappe vers frame présentée | 2,599 | 4,507 | **4,619** | 5,047 |
| frappe vers fin de paint | 2,408 | 4,293 | 4,412 | 4,851 |
| travail de la frame (render vers présentée) | 0,547 | 0,670 | **0,736** | 1,089 |
| saut de défilement, travail de la frame à froid | 1,553 | 1,958 | 2,093 | 2,093 |
| saut de défilement vers frame présentée | 3,583 | 5,450 | 5,918 | 5,918 |

Les p99 des cinq exécutions de « frappe vers frame présentée » : 4,639 ; 4,619 ; 4,695 ; 4,609 ; 4,614 ms. Première frame peinte 257,6 ms (médiane) après la lecture du fichier : estimation des hauteurs des 14 442 lignes, création de la fenêtre et du device, première mise en forme.

Lecture : la frappe vers frame se décompose en attente du prochain rafraîchissement (au plus 4,17 ms à 240 Hz) plus le travail de l'application (p99 0,74 ms, max 1,09 ms). Le travail ne dépend pas de la taille de la note : seules les lignes visibles sont mises en forme (moins de 120 lignes par frame, vérifié par test), et l'insertion dans un `String` de 1 Mo plus le décalage de l'index de lignes restent sous la milliseconde.

### Critère de latence retenu (décision d'Arthur, 2026-10-31)

Le critère « frappe vers frame < 16 ms au p99 » se lit désormais ainsi : **travail de l'application, de la frappe à la frame présentée (retour de `Present` DXGI, Metal ou wgpu), attente du prochain vsync exclue, p99 < 16 ms, et chaque frappe apparaît dans la toute prochaine frame présentée.** L'attente du vsync dépend de l'écran (jusqu'à 16,7 ms à 60 Hz pour n'importe quel moteur) et sort donc de la mesure.

Mesures Windows rapportées à ce critère (médiane des cinq exécutions) :

- **Travail de l'application** (render vers frame présentée) : p50 0,547 ms, p95 0,670 ms, **p99 0,736 ms**, max 1,089 ms ; pire p99 des cinq exécutions 0,839 ms, pire max 1,201 ms. Soit plus de 20 fois sous la cible de 16 ms. Le coût de `replace_text_in_range` (insertion dans le buffer, décalage de l'index de lignes) s'exécute au moment de la frappe, avant le render : le benchmark le compte dans le segment d'attente au lieu de l'isoler. Il est borné par ce segment (voir le point suivant) ; le benchmark CI de EP-003 devra l'isoler.
- **Frappe dans la toute prochaine frame** : l'attente entre la frappe et le render qui l'affiche a un max de 4,170 / 4,435 / 4,518 / 4,480 / 4,688 ms selon l'exécution, pour un intervalle de rafraîchissement de 4,17 ms à 240 Hz. Une frame manquée donnerait au moins 8,33 ms : aucune des 1 000 frappes n'a manqué la frame suivante (le dépassement de 0,5 ms au plus vient de la gigue du signal vsync). La frappe marque la vue sale, la frame suivante la dessine et la présente : c'est la frame dont le benchmark relève le `Present`.
- Pour mémoire, de bout en bout à 240 Hz (vsync compris) : p99 4,619 ms. À 60 Hz, ce chiffre serait d'environ 17,2 ms au p99 (projection non mesurée), sans effet sur le critère retenu.

**Verdict sous Windows : critère tenu.**

### Vrai éditeur (EP-003, Windows, 2026-11-01)

`cargo run -p cerata --example editor_bench --release` reprend la méthode sur l'éditeur d'EP-003 (`crates/cerata/src/editor`) dans une vraie fenêtre : début de frame marqué par `Window::on_next_frame`, frame présentée par une tâche lancée depuis ce callback, rendu de la vue racine vérifié entre les deux. Sur cinq exécutions : frappe, travail de l'application p99 1,50 à 1,68 ms ; gestionnaire d'entrée isolé p99 1,88 à 2,14 ms ; aucune frappe hors de la frame suivante ; saut de défilement à froid p99 3,55 à 4,02 ms ; ouverture de la note de 1 Mo jusqu'à la première frame présentée 18,6 à 21,0 ms. Le critère retenu est tenu avec la même marge que le spike.

## Critères

| # | Critère | Preuve | Verdict |
|---|---|---|---|
| 1 | 1 Mo, H1 à H6 à leur taille, virtualisation, height map, latence (critère retenu ci-dessus) | tests `each_heading_level_is_laid_out_at_its_own_font_size`, `rendering_is_virtualized_on_a_large_note`, `height_map::tests::*` ; benchmark ci-dessus | **go sous Windows** (travail p99 0,74 ms, aucune frame manquée sur 1 000 frappes) ; Linux suivi par US-045, macOS par US-046 |
| 2 | `**`, `#`, URL de lien masqués hors sélection, révélés au contact | tests purs `markup::tests::markup_is_hidden_when_the_selection_does_not_touch_the_node`, `markup_is_revealed_as_soon_as_the_selection_touches_the_node`, `line_offset_is_applied_to_absolute_selections` ; test GPUI `typing_through_the_input_handler_reveals_and_relayouts` ; captures | **go** |
| 3 | widget image à sa hauteur mesurée exacte | test `block_image_is_exactly_as_tall_as_the_image` (hauteur dans la height map, écart entre la ligne et la suivante, et bornes réelles du widget peint via `debug_bounds`, toutes égales à 251 px et 286 px, non multiples de 30) ; `debug_assert` dans `layout_line` et dans `prepaint` (taille mesurée par la mise en page GPUI = hauteur réservée) | **go** |
| 4 | glisser à travers une plage masquée et le widget, plage du buffer attendue | test GPUI `drag_across_hidden_marks_and_an_image_selects_the_buffer_range` (appui, déplacement sur l'image, relâchement simulés ; sélection attendue de l'offset 0 au 5e octet de la ligne sous l'image, marqueurs `**` et URL compris ; marqueurs figés pendant le glisser, révélés au relâchement) ; sonde `windows-drag-probe.ps1` à la souris réelle, captures conformes | **go** |
| 5 | IME et touches mortes sur les quatre cibles | voir ci-dessous | **go sous Windows (MS-IME japonais, AZERTY)** ; Pinyin sous Windows non concluant par entrée injectée, à vérifier à la main ; Wayland et X11 suivis par US-045, macOS par US-046 |

Détail du critère 3 : la mise en page de GPUI (taffy) arrondit la taille des éléments au pixel logique. Un 1000×397 ramené à 720 px de large fait 285,84 px en théorie et 286 px une fois mis en page. La height map prend donc la taille arrondie au pixel, celle que le widget aura réellement ; elle n'est jamais arrondie à un multiple de la hauteur de ligne. Les positions d'écran sont des sommes `f64` converties en `f32` : le test les compare à 10⁻³ px près, la hauteur elle-même est exacte.

Détail du critère 4 : un offset affiché au bord d'une plage masquée saute la plage vers l'avant (le premier caractère visible qui suit). Une sélection qui finit après « bold » inclut donc le `**` fermant et un clic au début d'un titre tombe après le `# ` masqué. À rediscuter dans EP-003 (affinité du caret au bord d'un marqueur).

## IME

### Statut par cible

| Cible | Chemin GPUI | Statut | Preuve |
|---|---|---|---|
| Windows, MS-IME japonais | IMM32 (`gpui_windows/src/events.rs:713-764`) | **mesuré ici, fonctionne** | `windows-input-probe.ps1` : disposition `00000411` activée, IME allumé par `VK_IME_ON`, saisie de `nihongo` au clavier (scancodes), espace, entrée. Journal : `replace_and_mark_text_in_range` reçoit `ｎ`, `に`, `にｈ`, `にほ`, `にほｎ`, `にほんｇ`, `にほんご`, puis `日本語` (conversion), puis `replace_text_in_range("日本語")` (validation). `bounds_for_range` est appelé au début de la composition et renvoie le caret (x 360,9, y 110,2, h 30) ; la capture montre le texte marqué souligné au caret et la fenêtre de candidats de MS-IME juste dessous. |
| Windows, AZERTY touche morte | `WM_DEADCHAR` puis `WM_CHAR` | **mesuré ici, fonctionne** | même sonde, disposition `0000040C`, scancode `0x1A` (`^` mort) puis `0x12` (`e`) : un seul `replace_text_in_range("ê")`, sans texte marqué. |
| Windows, Microsoft Pinyin | IMM32 | **non concluant, vérification humaine requise** | zh-CN ajouté à la liste des langues de l'utilisateur (Microsoft Pinyin, TIP `{81D4E9C9-1D3B-41BC-9E6C-4B40BF79E35E}`, déjà présent dans `System32\InputMethod\CHS` : ni téléchargement ni droits admin), puis liste restaurée à l'identique (exports JSON de `Get-WinUserLanguageList` avant et après identiques, `Preload` et profil en-US remis en état). `windows-input-probe.ps1 -ime pinyin` : Pinyin s'active (barre des tâches « 拼 ») mais reste en mode anglais (« 英 ») ; `zhongwen` puis `1` arrivent comme neuf `replace_text_in_range` de caractères simples, sans composition ni fenêtre de candidats. Basculements essayés sans effet : Maj (scancode puis VK), `VK_IME_ON`, Ctrl+Espace, `WM_IME_CONTROL` (`IMC_SETOPENSTATUS`, `IMC_SETCONVERSIONMODE` natif), valeur `Default Mode` = 0 posée puis retirée. Le blocage est en amont de l'application (mode de l'IME sous entrée injectée) : ni succès ni échec de GPUI n'est démontré. Le chemin IMM32 est celui de MS-IME japonais, qui fonctionne. |
| Linux Wayland, fcitx5 ou ibus | text-input-v3 | **en attente, vérification humaine** | aucune machine Linux de référence. |
| Linux X11, fcitx5 ou ibus | XIM | **en attente, vérification humaine** | idem. |
| macOS, saisie japonaise | NSTextInputClient | **en attente, vérification humaine** | aucune machine macOS de référence. |
| AZERTY touches mortes sur macOS et Linux | texte marqué puis validation | **en attente, vérification humaine** | le test automatique couvre la séquence d'appels attendue. |

Les sondes Windows envoient de vraies entrées clavier par `keybd_event` à travers la disposition de l'OS : c'est la chaîne complète (disposition, IME, messages Win32, GPUI, `EntityInputHandler`), mais pas un humain devant l'écran. La position visuelle des candidats a été contrôlée sur capture.

Tests automatiques (plateforme de test GPUI, tous OS) : `ime_composition_marks_text_at_the_caret_then_commits` (suite d'appels d'un IME, plage marquée en UTF-16, sélection relative au texte inséré, soulignement du texte marqué, `bounds_for_range` égal à la position du caret calculée indépendamment, validation qui efface la plage marquée) et `dead_key_sequences_produce_the_composed_character` (`^` marqué puis `ê` validé, et insertion directe de `ê` façon Windows).

Constat sur IMM32 : GPUI ne demande la position du caret qu'au `WM_IME_STARTCOMPOSITION` (`events.rs:647-672`). Une composition qui fait défiler la vue ne déplace donc pas la fenêtre de candidats. Sans conséquence ici (le texte marqué commence au caret) ; à garder en tête si le moteur fait défiler pendant une composition. TSF reste absent de `gpui_windows` (`PLAN.md` §7) ; MS-IME de Windows 11, qui est un IME TSF, fonctionne néanmoins par la couche de compatibilité IMM32.

### Liste de vérification manuelle

À exécuter sur chaque cible, build release, avec la note `image-check` de `windows-drag-probe.ps1` ou une note quelconque. Pour chaque étape, noter OK ou le comportement observé, et joindre une capture.

1. **Japonais** (MS-IME sous Windows, fcitx5-mozc ou ibus-mozc sous Wayland puis X11, saisie japonaise romaji de macOS). Placer le caret au milieu d'un paragraphe. Taper `nihongo` : le texte marqué « にほんご » apparaît au caret, souligné, et le reste de la ligne se décale. La fenêtre de candidats s'ouvre sous le caret, pas en haut à gauche de la fenêtre ni de l'écran. Espace : « 日本語 » remplace la composition, toujours souligné. Espace de nouveau : la liste de candidats s'ouvre au même endroit ; choisir le 2e candidat avec les flèches. Entrée : le texte est validé, plus de soulignement, caret après le texte. Échap pendant une composition : la composition disparaît sans rien insérer.
2. **Même séquence dans un titre H1** puis dans une ligne qui se replie (caret sur la 2e rangée) : la fenêtre de candidats suit la bonne rangée et la bonne taille de police.
3. **Pinyin** (Microsoft Pinyin sous Windows, fcitx5-chinese-addons ou ibus-libpinyin sous Linux, Pinyin de macOS) : taper `zhongwen`, choisir « 中文 » par le chiffre 1. Vérifier la position des candidats et le texte final.
4. **Touches mortes AZERTY** (disposition française de chaque OS) : `^` puis `e` donne `ê` ; `^` puis `^` donne `^` ; `¨` (Maj+`^`) puis `i` donne `ï` ; `^` puis `z` donne `^z`. Aucun caractère parasite, caret après le texte.
5. **Révélation** : pendant une composition dans un `**gras**`, les marqueurs restent visibles ; après validation et déplacement du caret hors du nœud, ils se masquent.
6. **Sous Wayland et X11**, recommencer 1 et 4 avec ibus puis fcitx5 si les deux sont installables ; noter la version du moteur.

## Limites du spike

Volontaires, sans effet sur la décision : buffer `String` au lieu d'une rope, height map en Fenwick avec reconstruction O(n) quand le nombre de lignes change (une `sum_tree` dans le vrai moteur), scanner écrit à la main (le parser est l'objet de US-003), pas d'undo, pas de flèches haut et bas, pas de clignotement du caret, lecture synchrone de l'en-tête d'image au premier affichage, police `.SystemUIFont` (Segoe UI sous Windows). Le benchmark ne compte ni la latence du compositeur ni celle de l'écran.

## Décision

**Go pour l'option C, et go pour EP-001** : un élément GPUI dédié, une mise en forme par ligne à sa propre taille, une height map ancrée, des décisions de masquage pures et des positions calculées depuis le modèle. Sous Windows, le critère de latence retenu est tenu avec plus d'un ordre de grandeur de marge (travail de l'application p99 0,74 ms contre 16 ms, aucune frappe hors de la frame suivante sur 1 000), les critères 2 à 4 sont prouvés par des tests, et l'IME japonais comme les touches mortes passent de bout en bout par `EntityInputHandler`.

Ce qui reste, hors de EP-001 :

- **Linux (Wayland et X11) : US-045, EP-007**, testé par Arthur. Même benchmark, au critère retenu ci-dessus, et liste de vérification IME. Rien dans le chemin mesuré n'est propre à Windows hors le rendu et `Present`, mais la mise en forme passe par cosmic-text, plus lent que DirectWrite sur les longues lignes.
- **macOS : US-046, EP-008**, sur un Mac Scaleway la semaine prochaine. Même benchmark et liste IME (saisie japonaise, touches mortes AZERTY) ; mise en forme par Core Text.
- **Pinyin sous Windows** (US-045) : étape 3 de la liste, à la main ; l'entrée injectée n'a pas pu faire passer Microsoft Pinyin en mode chinois.
- Seuils de réouverture de cette ADR : travail de l'application au-dessus de 8 ms au p99, ou frappe qui manque la frame suivante, sur une cible ; échec IME sur deux cibles ou plus. Un échec IME sur une seule cible ne remet pas en cause le moteur (le chemin est celui de GPUI, partagé avec Zed) mais ouvre une contribution upstream sur le backend concerné.
- La définition de la métrique de latence est tranchée (critère retenu ci-dessus) ; le benchmark CI de EP-003 la reprend et isole le coût du gestionnaire d'entrée.

Aucun critère n'est en no-go : EP-003 n'est pas bloqué. Si une réserve échoue, l'alternative reste celle de `PLAN.md` §3.4 : garder le portage Tauri pour l'éditeur, ou réduire le live preview (pas de tailles de titres, pas de masquage) dans un éditeur à hauteur uniforme.
