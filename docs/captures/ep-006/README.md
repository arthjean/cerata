# Captures et revue d'interface d'EP-006 (Windows)

## Revue `/better-interface` des tableaux éditables (2026-11-02)

Périmètre : les surfaces livrées par EP-006 dans l'éditeur. D'un côté, la géométrie et la peinture du tableau (US-049) : grille, en-tête, alignements, styles inline des cellules, `<br>`, `\|`, cellules vides et absentes, tableau dans une citation, zoom 200 % et fenêtre à sa largeur minimale. De l'autre, le caret, la sélection, la souris et le clavier dans une cellule (US-051, et US-052 pour le parcours au clavier) : clic, double et triple clic, glisser, frappe, révélation des marques, Tab, Entrée, flèches. Thème sombre par défaut, fenêtre de 1216 x 808 px. La composition d'un vrai IME (MS-IME japonais) a été vérifiée dans la même session, après la revue. Hors périmètre : les tableaux plus larges que la colonne de texte (défilement horizontal, EP-007).

`cerata.exe` (build debug) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`) sur un vault de démonstration (`Tables.md`, `Roadmap.md`, `Extra.md`). La capture « avant » vient d'une build du commit `e023b18` (fin d'EP-005), sur le même fichier. Les contrastes sont mesurés sur les pixels capturés. Les fichiers de `%APPDATA%\cerata` ont été sauvegardés avant la session et restaurés à l'identique après (vérifié par `diff -rq`). Le fichier de démonstration a été remis à l'octet près après les saisies (`cmp`).

Fixture `Tables.md` :

```markdown
| Feature | Left | Center | Right |
|:--|:--|:-:|--:|
| **Bold** and *italic* | `code` | ~~struck~~ | 42 |
| [A link](https://example.test) | [[Roadmap]] | &copy; -- :smile: | 3.14 |
| Two lines<br>in one cell | a \| b |  | 7 |
| A long cell that wraps across several lines of text because its column reaches the cap of forty-eight characters | short | mid | 1000 |
| Short row |

> | Quoted | Table |
> |---|---|
> | in | a quote |
```

Pile : GPUI épinglé par rev, géométrie du tableau dans `crates/cerata/src/editor/table.rs` (ADR 0007, décision 7). Les jetons de couleur sont dans `crates/cerata/src/editor/theme.rs` et les contrôles clavier partagés dans `crates/cerata/src/a11y.rs`. Il n'existe pas de documentation d'interface propre au projet en dehors des sources de Writer citées par le PRD et de l'ADR 0007. Les six skills de domaine ont été chargés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition.

| Domaine | Inspecté | Résultat |
|---|---|---|
| Accessibilité | parcours au clavier dans l'app (Tab, Entrée, flèches, Ctrl+M puis Tab), caret visible dans chaque état, tests GPUI d'interaction | Clear |
| Mise en page | captures à 100 %, à 200 % et à la largeur minimale de la fenêtre ; ligne trop courte, ligne trop longue, cellule vide, citation | 2 constats, dont 1 corrigé |
| Rédaction | chaînes ajoutées ou touchées par l'epic (`actions.rs`, `table.rs`, `cells.rs`), notice Ctrl+M dans un tableau | 1 constat |
| Typographie | graisse de l'en-tête, interligne des cellules (`CELL_LINE_HEIGHT` 1,4), colonne numérique alignée à droite | 1 constat |
| Couleur | contrastes mesurés sur pixels (voir Vérification), comparaison avec `e023b18` | Clear |
| Finition | rayon extérieur 8 px et rayon intérieur de l'en-tête 7 px (`table.rs`, `RADIUS - BORDER_WIDTH`), bordures, caret, rectangles de sélection | Clear |

### Constat corrigé

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| HIGH | Mise en page | `crates/cerata_markdown/src/table.rs` (`Grid::has_extra`), `crates/cerata_markdown/src/cells.rs` (`grid_in`), `crates/cerata_markdown/src/decorate.rs` (`table_revealed`) | Une ligne plus longue que l'en-tête (`\| 1 \| 2 \| hidden extra \|`) dessinait 2 cellules ; « hidden extra » n'apparaissait nulle part et le caret ne pouvait pas l'atteindre tant que le tableau restait dessiné (`revue-cellule-en-trop.png`). Seuls un triple clic ou un glisser hors du tableau la révélaient, et rien ne l'indiquait | Un tableau qui contient une cellule en trop sort du régime d'édition en place : `cells::grid_in` l'ignore (ni recalage ni règles de saisie) et sa source se révèle dès que la sélection le touche, comme dans Writer. Rendu inchangé tant qu'on n'y touche pas (`revue-cellule-en-trop-intact.png`) ; au clic, la source et la cellule en trop s'affichent, caret posé à l'endroit cliqué (`revue-cellule-en-trop-clic.png`). ADR 0007, décision 2, complétée | Déclencheur « contenu caché sans indice visible » : du texte du fichier était invisible pendant qu'on éditait sa ligne, et on pouvait supprimer la ligne sans le savoir. Tests : `a_table_with_cells_past_its_header_reveals_its_source_on_contact`, `a_table_with_extra_cells_is_edited_as_source`, `a_click_in_a_table_with_an_extra_cell_reveals_its_source` |

### Constats laissés ouverts

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| MEDIUM | Mise en page | `crates/cerata/src/editor/table.rs:529`, `:689` | Une cellule qui passe à plusieurs lignes dans une colonne centrée ou alignée à droite est dessinée alignée à gauche (`us-049-zoom-200.png`, cellule « &copy; – 😄 » de la colonne Center) | Peindre chaque ligne visuelle avec l'alignement de la colonne (`TextAlign` et largeur intérieure passés à `WrappedLine::paint`), et décaler d'autant `caret_rect`, `range_rects` et `offset_in_row` pour les cellules | L'alignement de la colonne se perd dès qu'une cellule passe à la ligne, ce qui arrive surtout au zoom 200 %. Déjà noté dans la section « Non prouvé » de l'ADR 0007 |
| LOW | Typographie | `crates/cerata/src/editor/table.rs` (`measure`, `layout`) | Colonne numérique alignée à droite en chiffres proportionnels (42, 3.14, 7, 1000) | Activer `tnum` (`FontFeatures`) sur les cellules d'une colonne alignée à droite | Chiffres tabulaires sur les colonnes numériques : les unités s'alignent d'une ligne à l'autre |
| LOW | Rédaction | `crates/cerata/src/editor/actions.rs:319` | « Tab now moves the focus. Press Ctrl+M to indent with Tab again. » | « Tab now moves the focus. Press Ctrl+M to use Tab in the text again. » | Dans un tableau, Tab ne met plus en retrait, il change de cellule : la notice décrit une action fausse à cet endroit |

### Constat hors epic

| Sévérité | Emplacement | Constat |
|---|---|---|
| LOW | `crates/cerata/src/editor/element.rs:727` (EP-003) | L'écouteur de `MouseUp` n'est posé qu'au frame qui suit l'appui. Si le relâchement arrive avant ce frame (clic de moins de 40 ms en build debug), il est perdu, et ce qui est révélé reste gelé sur la sélection d'avant le clic jusqu'au prochain mouvement de souris (`mouse.rs:359` termine alors le glisser). C'est observé avec un triple clic synthétique, qui ne révélait pas la source, et avec un appui de 90 ms, la révélation a lieu (`us-051-triple-clic.png`). Correctif : poser l'écouteur à chaque frame et laisser `end_drag` ignorer un relâchement sans glisser en cours |

### Non vérifié

- Lecture des cellules par un lecteur d'écran : l'éditeur n'expose pas encore son texte à AccessKit (US-044, EP-008).
- Thème clair et mode Contraste élevé de Windows.
- Zoom système de Windows à 200 % (seul le zoom de l'éditeur a été testé).
- Tableaux plus larges que la colonne de texte (EP-007).
- macOS et Linux (EP-009, EP-010).

### Vérification

Passé :

- Clic dans une cellule : le caret se pose à la position cliquée et le tableau reste dessiné (`us-051-clic-cellule.png`). Avant EP-006, le même clic sélectionnait le tableau entier et révélait sa source (`us-049-avant-clic.png`).
- Frappe de `X|` en fin de cellule : le fichier contient `*italic*X\|`, la cellule affiche `X|` (`us-051-frappe-pipe.png`).
- Caret dans « italic » : seules les marques `*` de cette cellule se révèlent (`us-051-marques.png`).
- Double clic : le mot sous le pointeur, borné à la cellule (`us-051-double-clic.png`). Glisser dans une cellule de deux lignes : rectangles de sélection dans la cellule seulement (`us-051-glisser-cellule.png`).
- Triple clic (appui de 90 ms) : le tableau entier est sélectionné et sa source révélée (`us-051-triple-clic.png`). Glisser d'une cellule jusqu'au paragraphe qui suit : la source se révèle (`us-051-glisser-hors.png`).
- Clic dans une cellule vide : caret à la hauteur d'une ligne, centré dans sa colonne (`us-051-cellule-vide.png`).
- Tab sélectionne le contenu de la cellule suivante (`us-052-tab.png`), Entrée celui de la cellule du dessous (`us-052-entree.png`). Bas descend de cellule en cellule ; vers une cellule absente, il va à la dernière cellule réelle de la ligne (`us-052-bas.png`). Bas depuis le paragraphe entre dans l'en-tête sans révéler la source (`us-049-etroit-200.png`).
- Ctrl+M puis Tab dans une cellule : le focus sort vers le bouton Retour, anneau visible, notice affichée (`revue-ctrl-m-tab.png`). Aucun piège au clavier.
- Zoom 200 % et fenêtre à sa largeur minimale (816 px) : les colonnes se resserrent jusqu'au mot le plus long, les cellules passent à la ligne, rien n'est coupé (`us-049-zoom-200.png`, `us-049-etroit-200.png`).
- `<br>` rend un saut de ligne, `\|` s'affiche `|`, `--` devient un tiret, `:smile:` un emoji ; le lien est en accent ; le lien wiki et le code inline ont le même style que dans un paragraphe, vérifié en saisissant le même contenu hors du tableau (`us-049-apres.png`). Avant, `<br>` s'affichait tel quel et le code avait un fond propre au rendu HTML (`us-049-avant.png`). `&copy;` reste du texte, dans un paragraphe comme dans une cellule.
- Contrastes mesurés sur pixels, thème sombre, fond de page et de cellule `#111111`, fond d'en-tête `#1d1d1d` : texte `#fcfcfc` sur cellule 18,4:1, sur en-tête 16,4:1 ; lien `#ff6a00` 6,6:1 ; lien wiki `#b1b1b1` 8,8:1 ; texte sur sélection `#582c0c` 11,5:1 ; caret `#ff6a00` 6,6:1. Bordure `#242424`, identique au rendu d'avant (`e023b18`).
- Tests : `cargo test --workspace` passe, dont les tests GPUI d'interaction du tableau (`crates/cerata/src/editor/tests.rs`, module `tables`).

- Après correctif, sur `Extra.md` relancé : grille intacte au repos, source et cellule en trop visibles au clic.
- IME (US-048, US-051) : fenêtre passée en japonais (`WM_INPUTLANGCHANGEREQUEST`, 0x0411), IME activé (`VK_IME_ON`), « nihon » tapé au clavier en fin de cellule. Le texte marqué s'affiche souligné dans la cellule et la liste de prédiction s'ouvre sous son caret (`us-048-ime-composition.png`) ; deux Espace ouvrent la liste de conversion au même endroit (`us-048-ime-candidats.png`) ; Entrée valide `二本` en place, le fichier contient `*italic*二本` et le caret reste dans la cellule (`us-048-ime-valide.png`). Clavier remis en français, saisie annulée.
- Recalcul des largeurs pendant la frappe (question ouverte du PRD) : 36 caractères tapés dans la colonne Left suffisent à resserrer la colonne Feature (sa cellule longue passe de 2 à 3 lignes), à renvoyer à la ligne la cellule de Center et à faire grandir le tableau de 45 px (`revue-largeurs-avant.png`, `revue-largeurs-apres.png`).

Verdict : Approve. Aucun constat HIGH ne subsiste dans le périmètre ; un MEDIUM et deux LOW restent à faire. Lecteur d'écran, thème clair et contraste élevé non vérifiés.

## Captures

| Fichier | Contenu |
|---|---|
| `us-049-avant.png`, `us-049-avant-clic.png` | Rendu au commit `e023b18`, puis le clic dans une cellule qui sélectionnait le tableau et révélait sa source |
| `us-049-apres.png` | Le même fichier mis en page par l'éditeur (EP-006) |
| `us-049-zoom-200.png`, `us-049-etroit-200.png` | Zoom 200 %, puis la fenêtre à sa largeur minimale, caret entré dans l'en-tête par Bas |
| `us-051-clic-cellule.png`, `us-051-frappe-pipe.png`, `us-051-marques.png` | Caret dans une cellule, `\|` tapé, marques révélées dans la seule cellule touchée |
| `us-051-double-clic.png`, `us-051-glisser-cellule.png`, `us-051-cellule-vide.png` | Mot sélectionné, sélection sur deux lignes d'une cellule, caret dans une cellule vide |
| `us-051-triple-clic.png`, `us-051-glisser-hors.png` | Tableau entier sélectionné et sélection sortie du tableau : la source se révèle |
| `us-052-tab.png`, `us-052-entree.png`, `us-052-bas.png` | Tab, Entrée, Bas |
| `revue-ctrl-m-tab.png` | Ctrl+M puis Tab : le focus quitte le texte |
| `us-048-ime-composition.png`, `us-048-ime-candidats.png`, `us-048-ime-valide.png` | MS-IME japonais dans une cellule : prédiction, conversion, validation |
| `revue-largeurs-avant.png`, `revue-largeurs-apres.png` | Le tableau avant et après une phrase tapée dans la colonne Left |
| `revue-cellule-en-trop.png` | Constat HIGH, avant correctif : la troisième cellule de `\| 1 \| 2 \| hidden extra \|` n'est pas dessinée |
| `revue-cellule-en-trop-intact.png`, `revue-cellule-en-trop-clic.png` | Après correctif : grille au repos, puis source révélée au clic avec la cellule en trop |
