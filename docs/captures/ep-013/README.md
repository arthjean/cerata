# Captures et revue d'interface d'EP-013 (Windows)

## Vérification des gestes dans l'app (2026-11-02)

`cerata.exe` (build debug de l'epic) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`) sur une copie de la fixture `crates/cerata/examples/fixtures/images/images.md`, ouverte seule (fenêtre compacte de 1216 × 808 px, thème sombre, zoom à 100 %, colonne de 734 px). Le curseur est lu par `GetCursorInfo` et comparé aux curseurs système (`IDC_SIZEWE`, `IDC_IBEAM`), les captures d'écran ne le montrant pas. Le fichier sur disque est relu après chaque geste (sauvegarde automatique).

| Geste | Entrée réelle | Résultat | Capture |
|---|---|---|---|
| Ouverture | la note seule | la capture de 1 600 px occupe la colonne, la PNG de 200 px est centrée (x 508 à 708 pour une colonne de 241 à 975) | `us064-centrage.png` |
| Survol | pointeur au centre de l'image | deux poignées à 8 px des bords, centrées verticalement, 48 px de haut | `us067-survol.png` |
| Clic | clic gauche au centre | sélection d'objet : anneau d'accent, poignées visibles, aucun caret ; le bord supérieur reste à y = 266 | `us065-clic-sans-saut.png` |
| Curseur | pointeur sur l'image, puis sur la poignée droite | `IBEAM`, puis `SIZEWE` | — |
| Geste et Échap | appui sur la poignée droite, 20 déplacements jusqu'en haut à gauche de la fenêtre (hors de l'image), Échap, relâchement | pendant le geste l'image rétrécit, centrée, les lignes suivantes remontent, le curseur reste `SIZEWE` hors de l'image ; Échap rend la largeur ; le fichier reste `![wide](wide.png)` | `us067-geste-hors.png`, `us067-echap.png` |
| Redimensionnement | appui sur la poignée droite, glisser de 217 px vers la gauche, relâchement | le fichier devient `![wide\|300](wide.png)` (734 − 2 × 217) | `us067-relache.png` |
| Menu contextuel | clic droit sur l'image | l'image est sélectionnée, le menu commence par « Image width » ; « Original size » est désactivé, l'image n'ayant pas de taille | `us068-menu.png` |
| Largeur prédéfinie | « Image width », puis « 50% » | `![wide\|367](wide.png)` ; Ctrl+Z rend `![wide](wide.png)` | `us068-50.png` |
| Retour arrière | clic sur l'image, Retour arrière | la ligne de l'image devient vide ; Ctrl+Z rend `![wide](wide.png)` | `us065-retour-arriere.png` |
| Double-clic | double-clic sur l'image | la source apparaît au-dessus de l'image, caret après `wide`, plus de poignées | `us065-double-clic.png` |
| Palette | Ctrl+Maj+P, « image width », deuxième résultat, Entrée | quatre commandes proposées, sans « Original size » ; le fichier devient `![wide\|367](wide.png)` | `us068-palette.png`, `us068-palette-50.png` |

**Défaut trouvé et corrigé pendant la vérification.** Sur le premier build, un clic sur l'image de 413 px faisait défiler la note de 19 px (`us065-saut-avant.png` : bord supérieur de 266 à 247). Le défilement automatique gardait la « ligne » du caret, l'image entière, dans les marges de sécurité. Un clic qui sélectionne une image comme objet ne fait plus défiler (`crates/cerata/src/editor/mouse.rs`, `select_image`). Régression : `a_click_on_an_image_near_the_bottom_does_not_scroll` (échoue sans le correctif). `us067-survol.png` et `us067-relache.png` viennent de ce premier build ; `us067-relache.png` montre donc encore le décalage de 19 px.

## Revue `/better-interface` de la surface d'EP-013

Périmètre : les poignées de redimensionnement (`crates/cerata/src/editor/resize.rs`, `image_handles`), l'anneau de sélection d'objet (`crates/cerata/src/editor/element.rs`, `RING_OFFSET`, `RING_WIDTH`), le sous-menu « Image width » (`crates/cerata/src/editor/menu.rs`, `body_items`) et les commandes de palette « Image width: … » (`crates/cerata/src/palette.rs`, `resize.rs`, `Preset`). Pile : GPUI épinglé par rev, jetons de couleur dans `crates/cerata/src/editor/theme.rs`. Documents d'interface du projet : les revues d'EP-006, EP-007 et EP-012, l'ADR 0008 ; pas de guide d'interface propre au projet. Les six skills de domaine ont été chargés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition. Hors périmètre : le thème clair, non capturé.

| Domaine | Inspecté | Résultat |
|---|---|---|
| Accessibilité | poignées (non focalisables, sans rôle, hors de l'arbre d'accessibilité) ; alternative sans glisser (SC 2.5.7 : sous-menu et palette) ; chemin clavier (palette, Maj+Fin et Maj+Origine sélectionnent l'image) ; zone de clic des poignées 24 × 48 px (SC 2.5.8, test `dragging_a_handle_resizes_the_image_in_one_undo_step`) ; « Original size » désactivé et non proposé dans la palette | Clear |
| Mise en page | centrage mesuré sur capture ; poignées symétriques à 8 px des bords ; anneau à 4 px hors de l'image, dans la marge latérale de 40 px ; sous-menu en tête du menu, séparé des entrées de texte | Clear |
| Rédaction | libellés « Image width », « 25% », « 50% », « 75% », « Full width », « Original size » et « Image width: … » | 1 constat ouvert |
| Typographie | aucun style ajouté : menu et palette existants (13 px) | Clear |
| Couleur | mesuré sur pixels : anneau `#FF6A00` sur `#111111` 6,58:1 ; poignée : bordure `#E8E9EA` sur l'image bleue `#466EB4` 4,16:1, remplissage sur blanc 11,0:1, bordure sur noir 16,8:1 (SC 1.4.11 : 3:1) | Clear |
| Finition | apparition des poignées sans animation (interaction fréquente) ; rayons de l'anneau et de l'image | 1 constat corrigé |

### Constat corrigé

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| LOW | Finition | `crates/cerata/src/editor/element.rs` (peinture des anneaux) | `quad(ring, px(6.0), …)` | `quad(ring, px(4.0 + RING_OFFSET), …)`, soit 8 px | Le rayon extérieur vaut le rayon intérieur (4 px, `images.rs`) plus l'écart : un anneau de 6 px n'était pas concentrique avec les coins de l'image |

### Constat laissé ouvert

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| MEDIUM | Rédaction | `crates/cerata/src/editor/resize.rs` (`Preset::command_label`) | « Image width: 50% » en casse de phrase, imposée par le PRD, à côté de libellés en casse de titre (« Create New File ») | La politique de casse de la palette, ouverte depuis la revue d'EP-007 (`palette.rs:93`) | Deux casses dans une même liste ; même cause racine que le constat ouvert d'EP-012 |

### Vérification

- Passés : les gestes du tableau ci-dessus dans l'app ; curseur `SIZEWE` sur la poignée et pendant le geste hors de l'image, `IBEAM` sur l'image ; contrastes mesurés sur `us065-clic-sans-saut.png` et `us067-survol.png` ; tests GPUI d'EP-013 (`crates/cerata/src/editor/tests.rs`, `crates/cerata/src/shell/tests.rs`).
- Non vérifiés : thème clair ; lecteur d'écran (annonce de la sélection d'objet) ; l'anneau à 8 px de rayon dans l'app (correctif postérieur aux captures, une valeur de rayon) ; le rendu par Obsidian des largeurs écrites (vérification manuelle du DoD, ci-dessous).

### Verdict

`Approve` : aucun constat `HIGH`. Thème clair non revu.

## Lecture par Obsidian (2026-11-02)

Vérification du DoD faite par l'agent, sur l'état final de l'epic.

**Écriture par Cerata.** `cerata.exe` (build debug de l'état final) est ouvert sur une copie de la fixture, dans un dossier temporaire. Les largeurs sont posées par de vraies entrées de l'OS, un chemin d'entrée par image. Le fichier est relu sur disque après la sauvegarde automatique.

| Image | Geste dans Cerata | Texte écrit | Largeur dans Cerata | Largeur dans Obsidian |
|---|---|---|---|---|
| `wide.png` (1 600 px) | clic sur l'image, palette « Image width: 50% » | `![wide\|367](wide.png)` | 367 px | 367 px |
| `small.png` (200 px) | glisser de la poignée droite, +60 px | `![small\|320](small.png)` | 320 px (agrandie) | 320 px (agrandie) |
| `diagram.svg` (320 px) | caret dans la source, palette « Image width: 75% » | `![diagram\|551](diagram.svg)` | 551 px | 551 px |
| `pic.png` (640 px, embed) | clic droit, « Image width », « 25% » | `![[pic.png\|184]]` | 184 px | 184 px |

Largeurs dans Cerata mesurées sur `cerata-largeurs.png` et `cerata-largeurs-2.png` (colonne de 734 px).

**Lecture par Obsidian.** Obsidian 1.14.5 (installeur 1.12.7) ouvre le même dossier comme coffre, en Live Preview. Les largeurs sont lues dans le DOM par le CLI d'Obsidian (`obsidian eval`) : attribut `width` et largeur de la boîte de chaque `img`. Résultat : 4 sur 4 identiques, et le texte alternatif affiché est `wide`, `small`, `diagram`, sans le suffixe. Captures `obsidian-largeurs.png` et `obsidian-largeurs-2.png`. Le coffre temporaire a ensuite été retiré de la liste des coffres d'Obsidian.

**Hauteur `|WxH` (question ouverte du PRD).** Écrits à la main, `![wide|300x100](wide.png)` et `![[pic.png|200x50]]` s'affichent dans Obsidian à 300 × 100 et 200 × 50 px, déformés (`object-fit: fill`). Cerata lit la hauteur et l'ignore (ADR 0008) : il affiche ces images à 300 × 169 et 200 × 125 px. Cerata n'écrit jamais de hauteur, donc ses propres fichiers ne sont pas concernés. L'écart ne touche que les `|WxH` écrits ailleurs.
