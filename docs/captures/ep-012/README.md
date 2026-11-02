# Captures et revue d'interface d'EP-012 (Windows)

## Vérification des gestes d'insertion (2026-11-02)

`cerata.exe` (build debug de l'epic) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`, presse-papiers Windows) sur une copie de la fixture `crates/cerata/examples/fixtures/images/images.md`, ouverte seule (fenêtre compacte, racine = dossier de la note). Les fichiers insérés sont hors de cette racine, donc copiés dans `images-assets/`. Fenêtre de 1216 × 808 px, thème sombre, zoom à 100 %.

| Geste | Entrée réelle | Résultat | Capture |
|---|---|---|---|
| Coller une capture | presse-papiers au format `PNG` (celui de l'Outil Capture d'écran), Ctrl+V en fin de titre | un bloc `![image.png](images-assets/20261102-222526-82b5.png)` sous le titre, caret sur la ligne vide qui suit ; Ctrl+Z le retire en une étape | `us060-capture-collee.png` |
| Coller un fichier copié | `Set-Clipboard -Path shot-b.jpg` (`CF_HDROP`), Ctrl+V | `![shot-b](images-assets/shot-b.jpg)` en bloc, extension du format détecté | non conservée (l'image collée montre un espace de travail privé) |
| Choisir 2 images au sélecteur | palette, « Insert image… », dialogue Windows libellé « Insert », `"shot-a.png" "diagram.svg"` | deux blocs séparés par une ligne vide, une étape d'annulation | `us062-palette.png`, `us062-selection.png` |
| Menu contextuel | clic droit dans le texte, survol d'Insert | « Image… » entre « Horizontal rule » et les dates, même casse que « Link… » | `us062-menu.png` |
| Déposer 3 fichiers depuis l'Explorateur | glisser réel de PNG, JPEG et SVG sélectionnés (Ctrl+A) dans l'Explorateur, relâché sous le premier paragraphe | barre d'accent de 2 px à la frontière de bloc pendant le survol ; trois blocs insérés à cette frontière ; collisions de noms numérotées (`shot-a 1.png`, puis `shot-a 2.png` au second essai) | `us061-barre.png`, `us061-depose.png` |
| Déposer un mélange d'images et de notes | glisser réel de `shot-b.jpg` et `mixed-note.md` | l'image est insérée dans la note (`![shot-b](<images-assets/shot-b 3.jpg>)`), la note déposée s'ouvre par la route actuelle du shell (la fenêtre compacte devient l'espace de travail de son dossier) | `us061-depot-mixte.png` |

Aucun fichier n'a été écrit hors de la racine : seul `images-assets/` a été créé, à côté de la note.

## Revue `/better-interface` de la surface d'insertion

Périmètre : la barre d'insertion du dépôt (`crates/cerata/src/editor/mod.rs`, rendu ; `crates/cerata/src/editor/import.rs`, `drop_boundary`), les notices d'import (`crates/cerata_core/src/images.rs`, `Refusal` et `refusal_notice` ; `editor/import.rs`, `loading_notice`), la commande de palette « Insert image… » (`crates/cerata/src/palette.rs`) et l'entrée « Image… » du menu contextuel (`crates/cerata/src/editor/menu.rs`). Pile : GPUI épinglé par rev, jetons de couleur dans `crates/cerata/src/editor/theme.rs`. Documents d'interface du projet : les revues d'EP-006 et d'EP-007 ; pas de guide d'interface propre au projet. Les six skills de domaine ont été chargés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition. Hors périmètre : le thème clair, non capturé.

| Domaine | Inspecté | Résultat |
|---|---|---|
| Accessibilité | chemin clavier de chaque geste (palette, Ctrl+V) ; alternative au glisser (SC 2.5.7 : sélecteur et collage) ; notice en `Role::Status` | Clear |
| Mise en page | barre à la frontière de bloc, jamais dans un bloc de code (test) ; notice centrée en bas de l'éditeur | Clear |
| Rédaction | les six notices, « Insert image… », « Image… », libellé « Insert » du sélecteur | 1 constat corrigé, 1 ouvert |
| Typographie | points de suspension typographiques (`\u{2026}`) dans les libellés et la notice de chargement | Clear |
| Couleur | barre `theme::accent()` mesurée sur pixels : `#FF6A00` sur `#121212`, 6,5:1 (SC 1.4.11 : 3:1) | Clear |
| Finition | aucune animation ajoutée ; barre sans rayon ni ombre, comme le caret | Clear |

### Constat corrigé

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| HIGH | Rédaction | `crates/cerata_core/src/images.rs` (`NoteContext::plan`, octets collés) | Une capture copiée comme bitmap (`CF_DIB`, que GPUI lit en BMP non compressé) dépasse vite 5 Mo : une capture de 1452 × 1028 px est refusée avec « Image not pasted: 5.7 MB is over the 5 MB limit » (`us060-bitmap-refuse-avant.png`), une erreur sans issue pour un geste courant | Un BMP collé est réencodé en PNG sans perte, hors du thread de l'interface, avant la limite de 5 Mo ; le texte de la notice et la limite restent ceux du PRD pour les autres formats | Une erreur qui ne propose aucune issue, sur le geste « coller une capture » du DoD. Tests : `pasted_bytes_keep_their_stamp_alt_and_limit` (cœur) et `a_pasted_bitmap_over_the_limit_is_saved_as_png` (Ctrl+V dans l'éditeur) |

### Constat laissé ouvert

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| MEDIUM | Rédaction | `crates/cerata/src/palette.rs:95` | « Insert image… » en casse de phrase, imposée par le PRD, à côté de libellés en casse de titre (« Create New File ») | La politique de casse de la palette, ouverte depuis la revue d'EP-007 (`palette.rs:93`) | Deux casses dans une même liste ; même cause racine que le constat d'EP-007 |

### Vérification

- Passés : les six gestes ci-dessus dans l'app ; contraste de la barre mesuré sur `us061-barre.png` (pixels `255,106,0` sur `18,18,18`) ; tests GPUI d'EP-012 (`editor/tests.rs`, `shell/tests.rs`).
- Non vérifiés : thème clair ; lecteur d'écran (annonce de la notice) ; journal de frames pendant l'import de 20 Mo (`ZED_MEASUREMENTS=1`), qui demande de relancer l'app hors de l'instance en cours ; le collage d'un bitmap après le correctif dans l'app (l'instance en cours précède le correctif ; couvert par le test GPUI).

### Verdict

`Approve` : plus aucun constat `HIGH`. Thème clair non revu.

## Revue d'epic (2026-11-02)

**Défaut corrigé (bloquant).** Un dépôt insérait par `insert_images`, qui passe en ligne quand la position touche une liste, une citation ou un tableau. Or la barre se cale au début d'un bloc : déposer devant une liste donnait `![a](…)- item`, devant un tableau `![a](…)| A | B |`, ce qui cassait le bloc. `insert_image_blocks`, écrit pour ce cas, n'était appelé que par un test. La cible `ImageTarget::At` d'un dépôt insère désormais toujours en blocs (`crates/cerata/src/editor/import.rs`, `insert_when_done`). Régression : `a_drop_before_a_list_or_a_table_inserts_blocks` (échoue sans le correctif). Ajout : `closing_the_tab_during_a_copy_inserts_nothing` ferme l'onglet par `WindowView::close_tab` pendant la copie.

**Frames pendant l'import de 20 Mo.** Build release, `ZED_MEASUREMENTS=1` (`gpui_util::measure`, « frame duration »), copie de la fixture ouverte seule. Quatre PNG de 5,4 Mo (21,6 Mo) copiés par `Set-Clipboard -Path` (`CF_HDROP`), Ctrl+Fin puis Ctrl+V par `SendKeys`, journal lu 6 s après. 3 runs : 3 frames chacun, max 8,76 ms, 1,74 ms et 1,89 ms ; aucune frame au-delà de 16 ms. Les 4 fichiers sont copiés dans `images-assets/` et insérés en blocs.
