# ADR 0008 : blocs image, largeur Obsidian, sélection d'objet et redimensionnement symétrique

> Statut : **décidé** le 2026-11-02. Epic : EP-013 (release RI Images), stories US-063 à US-068. Amende l'ADR 0006 (conséquence « Tableaux, formules et HTML gardent la révélation de la source au contact », étendue ici aux images) et, pour les images de bloc, les FR-02 et FR-12 du PRD de portage (`tasks/prd-cerata-port.md:1548`, `:1558`). Références : `docs/research/images-notion.md` §4 et §5 ; aide Obsidian, sources Markdown de `obsidianmd/obsidian-help` (`en/Editing and formatting/Basic formatting syntax.md:216-225`, `en/Linking notes and files/Embed files.md:46-62`) ; forum Obsidian, défauts de la 1.12.4 et de la 1.13.0 ([fil](https://forum.obsidian.md/t/desktop-windows-v1-12-4-image-resizing-feature-breaks-basic-image-operations-in-live-preview/111735)).

## Contexte

Une image s'affichait à sa largeur naturelle, bornée par la colonne, alignée à gauche (`images.rs`, `element.rs`). Le suffixe `|400` d'un embed était lu puis jeté (`wiki.rs`). Un clic sur l'image sélectionnait toute sa plage. La décoration la voyait alors « touchée » et révélait sa source au-dessus d'elle : l'image descendait d'une ligne sous le pointeur.

Obsidian 1.12 redimensionne les images à la souris. Son forum en signale le coût : Backspace, Suppr et Ctrl+X cassés (1.12.4), flèches capturées par le redimensionneur (1.13.0), lien de l'image masqué, aucune saisie de taille exacte.

## Décision

1. **Largeur dans le texte, syntaxe Obsidian** (`cerata_markdown::image_size`).
   - Lecture : le texte alternatif est coupé au dernier `|` non échappé ; un suffixe de la forme `^[0-9]{1,5}(x[0-9]{1,5})?$`, avec une largeur d'au moins 1, est une taille. Sinon, un alt entier de cette forme est une taille avec un alt vide (`![300](x.png)`). Un embed lit la sienne sur son dernier segment `|` (un `\|` sépare aussi, comme dans un tableau). `alt\|300`, `alt|300px`, `alt|50%`, `alt|0`, `alt| 300` et `alt|123456` ne sont pas des tailles.
   - Écriture : une fonction pure produit le `Change` qui pose, remplace ou retire la taille sans toucher au reste du texte. Un alt vide reçoit la forme documentée `![300](x.png)`. La hauteur d'un `|WxH` est lue, ignorée à l'affichage, jamais écrite.
   - Unité : le pixel logique, comme les tailles naturelles (le zoom n'agit pas sur les images).
2. **Rendu.** La largeur affichée vaut `min(W, colonne)`, ou `min(naturelle, colonne)` sans taille ; une `W` plus grande que la largeur naturelle agrandit l'image jusqu'à la colonne. La hauteur suit le rapport des largeurs. Une image en attente ou illisible qui porte `W` réserve une boîte de `W` de large. Une `<img width>` de bloc HTML garde son comportement.
3. **Centrage.** Les images de bloc plus étroites que la colonne sont centrées, comme dans Notion, y compris celles sans taille (changement de rendu accepté par le PRD).
4. **Sélection d'objet** (amende l'ADR 0006). Une plage de sélection exactement égale au nœud d'une image de bloc (Markdown ou embed), sans autre plage qui la touche, garde l'image dessinée. Un anneau de 2 px aux couleurs de l'accent l'entoure à 2 px de ses bords ; ni surlignage ni caret. Effets :
   - un clic sélectionne l'image comme objet, quelle que soit la sélection précédente, et l'image ne bouge pas ; un clic droit la sélectionne avant d'ouvrir le menu ;
   - Backspace, Suppr et Ctrl+X suppriment son Markdown, Ctrl+C le copie, un caractère tapé la remplace, chaque fois en une étape d'annulation : la sémantique texte de la sélection est gardée ;
   - le double-clic révèle la source, le caret à la fin du texte alternatif, avant un suffixe `|W` ;
   - les flèches entrent dans la source comme avant (`revealBlockOnArrow`) ; Maj+Fin depuis le début de la ligne, ou Maj+Origine depuis sa fin, sélectionnent l'image comme objet.

   Une plage qui touche l'image sans lui être égale, une image inline et une image de cellule gardent la règle de révélation. Une image n'est **pas** un bloc atomique au sens de l'ADR 0006 : la source reste joignable au clavier, ce qui répond aux défauts d'Obsidian 1.12.
5. **Redimensionnement symétrique.** Deux poignées, enfants de l'élément de l'image (`editor/resize.rs`), à 8 px des bords gauche et droit. Elles apparaissent au survol (`group_hover` du cadre de l'image) et tant que l'image est sélectionnée comme objet. Elles sont absentes pour une image inline, en attente ou indisponible, quand la source est révélée, pendant une sélection au glisser ou un dépôt de fichiers. Leur appui arrête la propagation (le caret ne bouge pas) et pose `Drag::Resize`. Pendant le geste :
   - la largeur change de deux fois le déplacement horizontal, l'image reste centrée, bornée entre 64 px et la colonne ;
   - seule la vue change (`LayoutCx::image_preview`) : le buffer, la révision et l'historique ne bougent pas, aucune sauvegarde n'est planifiée ;
   - le curseur `ResizeLeftRight` est tenu par `set_window_cursor_style`, et le pointeur capturé (`capture_pointer`).

   Au relâchement, une transaction `UserEvent::Command` écrit la largeur affichée, soit une étape d'annulation, si la source de l'image est restée la même. À moins de 8 px de `min(naturelle, colonne)`, la taille est retirée (aimant). Échap pendant le geste remet la largeur d'avant et le relâchement n'écrit rien.
6. **Alternative sans glisser** (WCAG 2.2 SC 2.5.7). Le sous-menu `Image width` du menu contextuel et les commandes de palette `Image width: 25%` et suivantes écrivent 25, 50 ou 75 % de la colonne, la colonne entière, ou retirent la taille (`Original size`, désactivé sans taille), avec le même aimant.

## Options écartées

- **`<img width>`** comme format d'écriture : fait basculer l'image dans le moteur des blocs HTML et réécrit bien plus que le suffixe.
- **Largeur relative (pourcentage)** : Obsidian ne la lit pas.
- **Image atomique** (ADR 0006) : rendrait la source inaccessible au clavier, le défaut même d'Obsidian 1.12.
- **Hit-test des poignées dans les gestionnaires de l'éditeur** (comme `Drag::Thumb`) : repli prévu si les enfants de l'élément image recevaient mal survol, curseur ou clic. Non retenu : le spike d'US-066 montre qu'ils les reçoivent.
- **Redimensionner la seule boîte de l'image pendant le geste et remettre en page au relâchement** : repli prévu si l'aperçu dépassait 16 ms. Non retenu : la remise en page complète tient le budget.

## Mesures (spike US-066, 2026-11-02)

- **Acheminement.** Les poignées sont des enfants du cadre de l'image, mis en page par `layout_as_root` et `prepaint_at`. Une poignée cachée ne reçoit rien et l'appui sélectionne l'image ; survolée, elle apparaît dans la frame qui suit et son appui démarre le geste sans toucher à la sélection (`editor/tests.rs`, `handles_show_on_hover_or_while_the_image_is_selected`, `dragging_a_handle_resizes_the_image_in_one_undo_step`). Le curseur `ResizeLeftRight` passe devant l'IBeam de l'éditeur, posé avant la peinture des blocs ; la plateforme de test n'expose pas le curseur actif, d'où une vérification dans l'app.
- **Latence.** `editor_bench`, scénario « redimensionner une image » : 3 × 60 largeurs d'aperçu sur la figure la plus proche du milieu de la note de 1 Mo, sans édition du buffer, lignes suivantes remises en page dans la frame. Build release, machine Windows de référence : p99 du travail de l'application 2,00 à 2,31 ms, max 2,35 ms, aucune largeur hors de la frame suivante, révision inchangée (`docs/benchmarks.md`).

Décision : la conception directe (enfants interactifs, aperçu qui remet en page) est retenue, sans repli.

## Lecture par Obsidian (DoD d'EP-013, 2026-11-02)

Obsidian 1.14.5 affiche, en Live Preview, les 4 images de bloc de la fixture aux largeurs écrites par Cerata (`|367`, `|320`, `|551`, `![[pic.png|184]]`, mesurées dans le DOM). Il applique aussi la hauteur d'un `|WxH`, en déformant l'image (`object-fit: fill`). Cerata garde la décision d'ignorer cette hauteur à l'affichage : un `|WxH` écrit par un autre outil s'affiche donc avec une hauteur différente dans les deux éditeurs. Détails : `docs/captures/ep-013/README.md`.

## Conséquences

- Le comportement d'un clic sur une image s'écarte de Writer : l'oracle des décorations (`tests/decorate_oracle.rs`) traite à part la sélection égale à une image de bloc.
- Une image inline ou de cellule n'est pas dessinée aujourd'hui (sa source reste) : sa largeur est lue et portée par le widget, sans effet visible tant que le rendu inline n'existe pas.
- Le cache des hauteurs est indexé par la largeur demandée quand elle existe : une image avec taille garde une hauteur exacte avant d'être mesurée.

## Non prouvé

- L'annonce de la sélection d'objet et du redimensionnement par un lecteur d'écran.
- Les mêmes mesures sous macOS et Linux (EP-009, EP-010 du portage).
