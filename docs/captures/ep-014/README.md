# Captures et revue d'interface d'EP-014 (Windows)

## Vérification des gestes dans l'app (2026-11-03)

`cerata.exe` (build debug de l'epic) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`) sur une copie de la fixture `crates/cerata/examples/fixtures/images/images.md`, ouverte seule (fenêtre compacte de 1216 × 808 px, thème sombre, zoom à 100 %, colonne de 734 px). Le curseur est lu par `GetCursorInfo`, le presse-papiers par `System.Windows.Forms.Clipboard`, et le fichier sur disque est relu après chaque geste (sauvegarde automatique).

| Geste | Entrée réelle | Résultat | Capture |
|---|---|---|---|
| Survol | pointeur au centre de l'image large | la barre apparaît dans le coin supérieur droit, à 8 px des bords : Image width, Copy image, Open image, Edit source, Delete image | `us070-survol.png` |
| Curseurs | pointeur sur un bouton, sur la poignée droite, sur l'image | `HAND`, `SIZEWE`, `IBEAM` | aucune |
| Infobulle | pointeur immobile 2 s sur « Copy image » | infobulle « Copy image », le libellé du menu | `us070-infobulle.png` |
| Copy image (barre) | clic sur le bouton | le presse-papiers porte un PNG de 1600 × 900 px et 5 649 octets, la taille de `wide.png` ; le caret ne bouge pas | aucune |
| Image width (barre) | clic sur le bouton, puis « 50% » | le sous-menu s'ouvre sous le bouton ; le fichier devient `![wide\|367](wide.png)`, le caret reste en fin de paragraphe, la barre disparaît quand l'image rétrécit sous le pointeur | `us070-largeurs.png`, `us070-50.png` |
| Image étroite | poignée droite de `small.png` tirée jusqu'à 64 px, puis survol | `![small\|64](small.png)` ; la barre s'aligne sur le bord droit de l'image et s'étend vers la gauche | `us070-etroite.png` |
| Menu contextuel | clic droit sur l'image | l'image est sélectionnée ; le menu commence par Image width, Copy image, Open image, Edit source, Delete image, puis Cut, Copy, Paste | `us069-menu.png` |
| Open image (menu) | « Open image » | confirmation FR-14 « Open this file in its default app? » avec le chemin ; Cancel n'ouvre rien | `us069-ouvrir.png` |
| Delete image (barre) | clic sur le bouton, puis Ctrl+Z | la ligne de l'image devient vide, `wide.png` reste sur disque ; Ctrl+Z rend `![wide\|367](wide.png)` | `us070-supprimer.png` |
| Sélection d'objet et Échap | clic sur l'image, pointeur hors de l'image, Échap | la barre reste avec la sélection, Échap la ferme, l'image reste sélectionnée | `us070-selection.png`, `us070-echap.png` |
| Retour après Échap | pointeur de nouveau sur l'image | la barre réapparaît | `us070-retour.png` |
| Edit source (barre) | clic sur le bouton | la source apparaît, caret après `wide`, avant `\|367` | `us070-source.png` |
| Palette | Ctrl+Maj+P, « image » | Copy image, Open image, Edit image source et Delete image suivent les largeurs | `us069-palette.png` |

**Défauts trouvés et corrigés pendant la vérification.**

- Échap puis retour du pointeur : sur le premier build, une barre fermée par Échap pointeur hors de l'image ne revenait pas au survol, et le clic sur « Edit source » tombait sur l'image. GPUI tient aussi le survol pour terminé après une touche (`HoverListenerMode::InputModalityAware`), ce qui émettait une fausse sortie puis entrée. Les deux écouteurs de survol passent en `InputModalityIndependent`, et toute entrée ou sortie du pointeur rouvre la barre (`crates/cerata/src/editor/image_actions.rs`, `hover_image`). Régression : `the_toolbar_shows_on_hover_and_on_an_object_selection` (échoue sans le correctif).
- Survol figé après un défilement : le pointeur sur la barre de l'image large, Ctrl+Fin fait défiler la note sans événement de sortie ; l'embed survolé ensuite ne montrait pas sa barre (`us070-embed-avant.png`). Chaque frame oublie le survol d'une image ou d'une barre qu'elle n'a pas dessinée (`Toolbar::prune`, appelé par `crates/cerata/src/editor/element.rs`). Régression : `a_toolbar_gone_under_the_pointer_lets_the_next_image_show_its_own` (échoue sans le correctif). Le correctif n'a pas été revu dans l'app : la fenêtre de Cerata avait perdu le premier plan, et le pilotage par entrées de l'OS a été arrêté pour ne pas agir sur une autre fenêtre.
- Échap, pointeur sur la barre (revue d'epic) : la barre disparaissait puis revenait à la frame suivante. `Toolbar::prune` oubliait le survol de la barre, et l'image découverte sous le pointeur, survolée de nouveau, rouvrait la barre. Une barre fermée sous le pointeur reste fermée jusqu'à ce qu'il sorte (`Toolbar::held`). Régression : `escape_on_the_toolbar_closes_it_until_the_pointer_leaves` (échoue sans le correctif). Non revu dans l'app.
- L'infobulle « Image width » couvrait le haut du sous-menu ouvert sous le bouton : les boutons n'ont plus d'infobulle tant qu'un menu est ouvert (`us070-largeurs.png`, après correctif).

## Revue `/better-interface` de la surface d'EP-014

Périmètre : la barre d'outils au survol (`crates/cerata/src/editor/image_actions.rs`, `image_toolbar`, `Toolbar` ; cadre de l'image dans `crates/cerata/src/editor/element.rs`), la section Image du menu contextuel (`crates/cerata/src/editor/menu.rs`, `body_items`, `width_items`), les commandes de palette « Copy image », « Open image », « Edit image source », « Delete image » (`crates/cerata/src/palette.rs`) et les notices d'action (`image_actions.rs`, `not_found`, `unreadable`). Pile : GPUI épinglé par rev, jetons de couleur dans `crates/cerata/src/editor/theme.rs`, icônes Hugeicons (`crates/cerata/src/icons.rs`). Documents d'interface du projet : les revues d'EP-006, EP-007, EP-012 et EP-013, l'ADR 0008 ; pas de guide d'interface propre au projet. Les six skills de domaine ont été chargés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition. Hors périmètre : le thème clair, non capturé.

| Domaine | Inspecté | Résultat |
|---|---|---|
| Accessibilité | boutons `a11y::button` (rôle, nom égal au libellé du menu, anneau de focus à la couleur du texte) ; cibles de 28 × 28 px (SC 2.5.8, test `the_toolbar_shows_on_hover_and_on_an_object_selection`) ; chemin clavier de chaque action par la palette (SC 2.1.1, test `the_palette_offers_the_actions_of_the_image_at_the_caret`) ; contenu au survol (SC 1.4.13 : Échap la ferme, le pointeur peut s'y poser, elle reste tant qu'il y est) ; Delete image annulable en une étape | Clear |
| Mise en page | barre à 8 px des bords mesurée ; image étroite et éditeur plus étroit que la barre (test `the_toolbar_of_a_narrow_image_grows_to_the_left`) ; sous-menu sous son bouton ; section Image en tête du menu, séparée des entrées de texte | 1 constat ouvert |
| Rédaction | libellés du menu et des infobulles, libellés de palette, trois notices | 1 constat corrigé, 1 ouvert |
| Typographie | aucun style ajouté : menu existant (13 px), infobulle existante (`math::tooltip`) | Clear |
| Couleur | mesuré sur `us070-survol.png` : icône `#EFEFEF` sur la barre `#111111` 16,42:1 ; barre sur l'image bleue `#466EB4` 3,73:1 (SC 1.4.11 : 3:1) ; survol d'un bouton `#282828` sur `#111111` 1,28:1, état non requis, jeton `row_hover` du projet | Clear |
| Finition | rayons concentriques ; apparition sans animation (interaction fréquente) ; icônes en `currentColor`, trait de 1,5 px comme le reste du jeu | 1 constat corrigé |

### Constats corrigés

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| HIGH | Rédaction | `crates/cerata/src/editor/image_actions.rs` (`not_found`, `unreadable`) | « Image not found: wide.png », « Can't copy pic.svg », « Can't copy pic.svg: {erreur de l'OS} » | « Image not found: wide.png. Restore the file or edit the image's source. » et « Can't copy pic.svg: the file can't be read as an image. Check it or edit the image's source. » | Une erreur qui ne nomme aucune issue (déclencheur). Le texte du PRD reste en tête de la notice ; l'erreur de l'OS, localisée en français, ne se mêle plus au texte anglais. Tests : `copy_image_puts_the_file_on_the_clipboard`, `open_image_follows_fr_14` |
| LOW | Finition | `crates/cerata/src/editor/image_actions.rs` (`image_toolbar`) | `.rounded(px(6.0))` | `.rounded(px(4.0 + PAD + 1.0))`, soit 8 px | Rayon extérieur = rayon des boutons (4 px) + marge intérieure (3 px) + bordure (1 px) ; les captures précèdent ce correctif |

### Constats laissés ouverts

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| MEDIUM | Rédaction | `crates/cerata/src/editor/image_actions.rs` (`ImageAction::command_label`) | « Copy image », « Edit image source » en casse de phrase à côté de « Create New File » | La politique de casse de la palette, ouverte depuis la revue d'EP-007 (`palette.rs`) | Deux casses dans une liste ; même cause racine que les constats ouverts d'EP-012 et EP-013 |
| LOW | Mise en page | `crates/cerata/src/editor/image_actions.rs` (`image_toolbar`, `INSET`) | Sur une image de moins de 54 px de haut (`us070-etroite.png` : 64 × 38 px), la barre de 36 px couvre l'image | Pas de correctif proposé : la barre ne vit qu'au survol, Échap ou la sortie du pointeur la retire | L'image reste lisible hors survol ; changer l'ancrage pour les seules petites images ajouterait une règle |

### Vérification

- Passés : les gestes du tableau ci-dessus dans l'app ; curseurs `HAND`, `SIZEWE`, `IBEAM` ; contenu du presse-papiers après Copy image ; contrastes mesurés sur `us070-survol.png` et `us070-infobulle.png` ; tests GPUI d'EP-014 (`crates/cerata/src/editor/tests.rs`, `crates/cerata/src/shell/tests.rs`) et `cargo test --workspace --locked`.
- Non vérifiés : thème clair ; lecteur d'écran (annonce des boutons) ; Open image sur une image `https:` dans l'app (couvert par `open_image_follows_fr_14`, sans lancer le navigateur) ; la barre d'une image illisible et le correctif du survol figé dans l'app (couverts par `the_toolbar_is_absent_where_it_does_not_apply` et `a_toolbar_gone_under_the_pointer_lets_the_next_image_show_its_own`) ; le rayon de 8 px dans l'app (correctif postérieur aux captures, une valeur) ; le collage de l'image copiée dans une application qui ne lit que `CF_DIB` (GPUI n'écrit que le format enregistré `PNG`).

### Verdict

`Approve` : aucun constat `HIGH` ne reste. Thème clair non revu.
