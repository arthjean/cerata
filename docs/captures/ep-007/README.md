# Captures et revue d'interface d'EP-007 (Windows)

## Revue `/better-interface` des tableaux éditables (2026-11-02)

Périmètre : les surfaces livrées par EP-007 dans l'éditeur. US-058 : largeurs de colonnes figées pendant la frappe, puis recalculées à la sortie du tableau. US-053 : sous-menu Table du menu contextuel et commandes de tableau dans la palette. US-054 : sélection rectangulaire à la souris et au clavier. US-056 : tableau plus large que la colonne de texte (défilement horizontal, barre, suivi du caret), recherche dans les cellules et « Edit as Markdown ». Thème sombre par défaut, fenêtre de 1216 × 808 px, zoom de l'éditeur à 100 % puis à 175 %. Hors périmètre : le presse-papiers de cellules (US-055), couvert par les tests GPUI, et la robustesse (US-057), couverte par le fuzz et `editor_bench`.

`cerata.exe` (build debug) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`) sur un vault de démonstration (`Tables.md`, ci-dessous). Les contrastes sont mesurés sur les pixels capturés. Les fichiers de `%APPDATA%\cerata` ont été sauvegardés avant la session et restaurés à l'identique après (vérifié par `diff -rq` ; la session avait écrit `editor.zoom = 175` et la liste des fichiers récents).

Fixture `Tables.md` :

```markdown
# Tables

| Feature | Left | Center | Right |
|:--|:--|:-:|--:|
| **Bold** and *italic* | `code` | ~~struck~~ | 42 |
| A long cell that wraps across several lines of text because its column reaches the cap | short | mid | 1000 |
| Tea | x | y | 7 |

Wide table:

| Id | Name | Description | Owner | Status | Region | Started | Notes |
|---|---|---|---|---|---|---|---|
| 1 | Atlas | Inventory service rewrite for the warehouse fleet | Dana Whitfield | in review | eu-west-3 | 2026-01-04 | blocked by the carrier API migration |
| 2 | Borealis | Search indexing pipeline | Kenji Okafor | shipped | us-east-1 | 2025-11-20 | measured at p99 under budget |
| 3 | Cobalt | Billing ledger reconciliation | Priya Laurent | draft | ap-south-1 | 2026-03-02 | needle in the last column |

End.
```

Pile : GPUI épinglé par rev ; géométrie du tableau dans `crates/cerata/src/editor/table.rs` (ADR 0007), menu contextuel dans `crates/cerata/src/editor/menu.rs`, palette dans `crates/cerata/src/palette.rs`, jetons de couleur dans `crates/cerata/src/editor/theme.rs`. Documents d'interface du projet : l'ADR 0007 et la revue d'EP-006 (`docs/captures/ep-006/README.md`) ; pas de guide d'interface propre au projet. Les six skills de domaine ont été chargés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition.

| Domaine | Inspecté | Résultat |
|---|---|---|
| Accessibilité | menu contextuel et sous-menu Table à la souris ; commandes de tableau au clavier par la palette (Ctrl+P) ; Tab, Maj+flèches, Fin dans les cellules ; tests GPUI d'interaction | 1 constat corrigé |
| Mise en page | sous-menu près du bas de la fenêtre ; tableau large à 100 % et à 175 % ; repères du contenu masqué ; recherche d'un résultat masqué | 3 constats corrigés, 2 ouverts |
| Rédaction | libellés du registre (`commands.rs`), sous-menu, palette, message de limite de collage | 1 constat |
| Typographie | retour à la ligne dans une colonne figée étroite, cellules de plusieurs lignes | 1 constat |
| Couleur | barre du tableau, rectangle de cellules, résultat courant de recherche, mesurés sur pixels | Clear dans le périmètre ; 2 constats hors epic |
| Finition | rayon et bordure du sous-menu (jeton partagé du menu), bordure droite du tableau, barre | Clear |

### Constats corrigés

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| HIGH | Accessibilité | `crates/cerata/src/editor/menu.rs:320-334` (liste) et `:389-422` (lignes de sous-menu) | Un clic sur un item d'un sous-menu fermait le menu sans exécuter la commande : `on_mouse_down_out` de la liste principale voyait l'appui hors de ses bornes (le sous-menu est dessiné à côté) et fermait le menu avant le clic. Aucune commande du sous-menu Table n'était utilisable à la souris ; Format, Paragraph et Insert (EP-005) avaient le même défaut. Les tests appelaient `run_menu_action` directement | Seule la liste principale ferme le menu, et pas quand le pointeur est sur le sous-menu ouvert (`ContextMenu::in_submenu`, suivi par `on_hover` du sous-menu, remis à zéro quand le sous-menu change). Clic réel dans l'app : « Insert column right » insère la colonne, caret dans la nouvelle cellule (`us053-colonne.png`) | Un chemin au pointeur annoncé par l'interface ne menait nulle part. Test : `a_click_in_the_table_submenu_runs_its_command` (`editor/tests.rs:1867`), qui échoue sans le correctif et vérifie aussi qu'un appui hors des deux listes ferme le menu |
| HIGH | Mise en page | `crates/cerata/src/editor/menu.rs:423-435` | Le sous-menu Table (21 commandes, 5 séparateurs) s'ouvrait à la hauteur de sa ligne et sortait de la fenêtre par le bas : de « Align right » à « Delete table », les commandes étaient coupées et inatteignables (capture non conservée) | Le sous-menu est placé par `anchored().snap_to_window_with_margin(px(8.0))` : il remonte et se décale pour tenir dans la fenêtre (`us053-sousmenu.png`) | Contrôles coupés et inatteignables : déclencheur HIGH |
| MEDIUM | Mise en page | `crates/cerata/src/editor/mouse.rs:540-570` (`reveal_in_block`) | Une occurrence trouvée au début d'une ligne visuelle d'une cellule (« Priya / Laurent ») restait à moitié cachée quand le tableau était défilé à droite : `position_for_index` place l'indice d'une coupure en fin de ligne précédente, et le défilement s'arrêtait trop tôt (`revue-recherche-avant.png`, « rent » seul visible) | L'abscisse est prise depuis le début de la ligne visuelle que donne `row_at` (`x_for_index(display) - x_for_index(row_start)`) ; l'occurrence s'affiche entière (`us056-recherche.png`) | Un résultat de recherche doit être visible en entier. Test `a_result_left_of_the_scrolled_table_shows_whole` (`editor/tests.rs:2093`) ; il passe aussi sans le correctif sous le système de texte des tests, la preuve est la capture |
| MEDIUM | Mise en page | `crates/cerata/src/editor/table.rs:488` (`whole_pixels`), `crates/cerata/src/editor/code_scroll.rs:102-108` (`thumb`) | Les largeurs de colonnes partagées pour remplir la colonne de texte étaient arrondies chacune au pixel supérieur : un tableau qui tient dépassait de quelques pixels, sa bordure droite était coupée et une barre de défilement apparaissait pour rien (`us058-apres.png`) | Arrondi au pixel puis retrait d'un pixel aux colonnes qui ont de la marge au-dessus de leur mot le plus long, tant que le tableau dépasse ; la barre n'apparaît qu'à partir d'un pixel de débordement, puisque les décalages sont des pixels entiers | Un faux repère de contenu masqué et une bordure coupée. Tests : `rounded_widths_never_overflow_a_table_that_fits` (`table.rs:968`), `a_narrower_block_goes_back_to_zero_and_loses_its_bar` (`code_scroll.rs:178`) |

### Constats laissés ouverts

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| MEDIUM | Typographie | `crates/cerata/src/editor/table.rs:772` (`shape_text` des cellules) | Dans une colonne figée étroite (US-058), un mot plus long que la colonne est coupé, et l'espace qui suit commence la ligne visuelle suivante : ligne vide après « ence », « ␣in » décalé (`us058-pendant.png`). Même effet au repos dans une colonne serrée (« ␣at p99 », `us056-repos.png`) | Retirer l'espace de tête d'une ligne visuelle de cellule (pendre l'espace en fin de ligne précédente), comme le fait le texte courant | Lecture hachée et une ligne vide qui fait grandir la ligne du tableau sans contenu |
| MEDIUM | Mise en page | `crates/cerata/src/editor/element.rs` (barre du tableau dans sa dernière ligne) | Le seul repère d'un tableau qui dépasse, outre la bordure absente du côté coupé, est la barre dessinée dans sa dernière ligne : dans un tableau plus haut que la vue, elle peut être hors écran (`us056-zoom-repos.png`, colonnes Region à Notes masquées) | Un dégradé vers le fond sur le bord coupé, à gauche et à droite selon le défilement | Le contenu masqué a un repère, mais faible quand la coupure tombe sur un filet de colonne |
| MEDIUM | Rédaction | `crates/cerata/src/palette.rs:93` | La palette nomme les commandes de tableau par leur libellé de sous-menu, en casse de phrase (« Sort ascending », `us053-palette.png`), à côté de libellés en casse de titre (« Create New File ») et sans l'objet qu'ils modifient | Un libellé de palette qui nomme l'objet, dans la casse de la palette : « Table: Sort Ascending » | Hors du sous-menu Table, « Sort ascending » ne dit pas ce qu'il trie ; deux casses dans une même liste |
| LOW | Mise en page | `crates/cerata_markdown/src/table_select.rs:82` (`select_cells`) | Un rectangle de cellules révèle la syntaxe de chaque cellule couverte (`` `code` ``, `~~struck~~`), et les lignes grandissent pendant la sélection (`us054-glisser.png`) | Ne pas révéler les marques des cellules d'un rectangle : il sélectionne des cellules, pas du texte | Saut de mise en page pendant une sélection ; le glisser, lui, garde les révélations figées jusqu'au relâchement |
| LOW | Mise en page | `crates/cerata/src/editor/mouse.rs:557` | Le caret amené dans la dernière colonne laisse cachés le reste de sa marge et la bordure droite du tableau (`us056-zoom-tab.png`) | Quand la cellule du caret est la dernière, défiler jusqu'au bout | Le bord du tableau reste invisible alors qu'il n'y a plus rien à droite |

### Constats hors epic

| Sévérité | Emplacement | Constat |
|---|---|---|
| HIGH | `crates/cerata/src/editor/theme.rs:96` (`search_current`, EP-003) | Texte `#fcfcfc` sur le résultat courant de recherche `#c47a0b` (mesuré, `rgba(0xf59e0bb0)` sur `#111111`) : 3,3:1, sous les 4,5:1 du critère 1.4.3. Le même jeton sert dans le texte courant et dans les cellules. Correctif : une opacité de 0,5 donne environ 6,1:1, à distinguer encore des autres résultats (`search_match`, 0,33) |
| MEDIUM | `crates/cerata/src/editor/theme.rs:90` (`scrollbar`, EP-003) | Barre `#3d3d3d` sur `#111111` : 1,7:1, sous les 3:1 du critère 1.4.11 pour un composant. Jeton partagé par la barre de l'éditeur, des blocs de code et des tableaux |

### Non vérifié

- Lecture des cellules et du menu par un lecteur d'écran : l'éditeur n'expose pas encore son texte à AccessKit (US-044, EP-008), et le menu contextuel est dessiné par l'éditeur sans rôle de menu.
- Navigation au clavier dans le menu contextuel : il n'en a pas (EP-003) ; les mêmes commandes passent par la palette.
- Thème clair, mode Contraste élevé de Windows, zoom système à 200 %.
- macOS et Linux (EP-009, EP-010).

### Vérification

Passé :

- US-058 : 38 caractères tapés dans la cellule « x » de la colonne Left : aucune colonne ne bouge, la cellule passe à la ligne et seule sa ligne grandit (`us058-avant.png`, `us058-pendant.png`). Clic hors du tableau : les colonnes sont recalculées, la colonne Left s'élargit (`us058-apres.png`, prise avant le correctif d'arrondi : bordure droite coupée et barre pour quelques pixels).
- US-053 : clic droit dans une cellule, survol de « Table » : le sous-menu entier tient dans la fenêtre, groupé par séparateurs (lignes, colonnes, alignement, tri, tableau) (`us053-menu.png`, `us053-sousmenu.png`). Clic sur « Insert column right » : colonne insérée, caret dans la nouvelle cellule (`us053-colonne.png`). Ctrl+P puis « sort » : la palette propose « Sort ascending » et « Sort descending » (`us053-palette.png`).
- US-054 : glisser de « code » à « y » : rectangle de 2 × 3 cellules peint (`us054-glisser.png`) ; Maj+Droite l'étend à la colonne suivante (`us054-maj-droite.png`). Texte `#fcfcfc` sur le fond du rectangle `#582c0c` : 11,5:1 (même jeton que la sélection de texte, mesuré à la revue d'EP-006).
- US-056 : à 175 %, le tableau large est coupé à droite, bordure absente (`us056-zoom-repos.png`) ; Tab de cellule en cellule fait défiler le tableau jusqu'à Notes (`us056-zoom-tab.png`). Après correctif, une recherche de « Laurent » le ramène et l'affiche entier (`us056-recherche.png`). « Edit as Markdown » depuis le sous-menu révèle la source sur le fond de code, caret en place (`us056-source.png`) ; un clic hors du tableau redessine la grille (`us056-source-quitte.png`).
- Tests : `cargo test -p cerata --bin cerata` passe, dont les tests ajoutés par cette revue.

Verdict : Approve. Aucun constat HIGH ne subsiste dans le périmètre d'EP-007 : les deux HIGH (clic dans un sous-menu, sous-menu coupé) sont corrigés. Trois MEDIUM et deux LOW restent à faire ; le contraste du résultat courant de recherche (HIGH) et celui de la barre (MEDIUM) relèvent d'un jeton d'EP-003, hors epic. Lecteur d'écran, thème clair et contraste élevé non vérifiés.

## Captures

| Fichier | Contenu |
|---|---|
| `us058-avant.png`, `us058-pendant.png`, `us058-apres.png` | Le tableau avant la frappe, pendant (colonnes figées), puis après la sortie du tableau |
| `us053-menu.png`, `us053-sousmenu.png`, `us053-colonne.png` | Menu contextuel dans une cellule, sous-menu Table calé dans la fenêtre, colonne insérée par un clic |
| `us053-palette.png` | Commandes de tableau dans la palette |
| `us054-glisser.png`, `us054-maj-droite.png` | Rectangle de cellules au glisser, étendu par Maj+Droite |
| `us056-repos.png` | Tableau large à 100 %, au repos, barre dans sa dernière ligne |
| `us056-zoom-repos.png`, `us056-zoom-tab.png` | À 175 % : tableau coupé à droite, puis défilé par Tab jusqu'à la dernière colonne |
| `revue-recherche-avant.png`, `us056-recherche.png` | Résultat de recherche à moitié caché avant correctif, puis entier |
| `us056-source.png`, `us056-source-quitte.png` | « Edit as Markdown », puis la grille redessinée à la sortie |
