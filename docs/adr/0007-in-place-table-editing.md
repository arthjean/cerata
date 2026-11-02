# ADR 0007 : édition des tableaux en place, le tableau reste dessiné et le caret vit dans ses cellules

> Statut : **décidé** le 2026-11-02. Epics : EP-006 et EP-007 (release R4). Amende la conséquence « Tableaux, formules et HTML gardent la révélation de la source au contact » de l'ADR 0006 pour les tableaux, et le premier critère d'US-032. Références : `crates/cerata/src/editor/mouse.rs:255-264`, `crates/cerata_markdown/src/decorate.rs:553-589` ; changelogs Obsidian [1.5.0](https://obsidian.md/changelog/2023-11-20-desktop-v1.5.0/), [1.5.2](https://obsidian.md/changelog/2023-12-12-desktop-v1.5.2/), [1.5.5](https://obsidian.md/changelog/2024-02-13-desktop-v1.5.5), [1.5.8](https://obsidian.md/changelog/2024-02-22-desktop-v1.5.8/) ; Obsidian 1.14.5 observé dans son bundle installé (`%APPDATA%\obsidian\obsidian-1.14.5.asar`), comportement seulement, aucun code repris.

## Contexte

Cerata reproduit Writer : un tableau replié se révèle en source monospace dès que la sélection le touche (`decorate.rs:553-589`), et un clic sur le widget sélectionne toute sa plage (`mouse.rs:255-264`, port de `fold/core.ts:176-203`). La grille laisse place à des lignes de `|` qui débordent, et une frappe juste après le clic remplace le tableau entier.

Obsidian édite les tableaux cellule par cellule depuis la 1.5.0 (2023-11-20). Dans la 1.14.5, la cellule active est un éditeur CodeMirror imbriqué dont chaque transaction est retraduite dans le document (`dispatchUpdate`). Un saut de ligne devient `<br>` et un `|` devient `\|`, la cellule affichant l'inverse. Ctrl+Z est redirigé vers l'historique du document, et toute la colonne est re-paddée à chaque frappe (`updateCell`, largeur en unités UTF-16, minimum 5). Un tableau « malformé » est réécrit en entier dès qu'on entre dans une cellule (`dispatchTable`). Ce pont et cette normalisation sont à l'origine de bugs publiés : espaces supprimés sur les grands tableaux (1.5.5), annulation qui ajoute des espaces ou corrompt des cellules ([forum 80581](https://forum.obsidian.md/t/table-undo-redo-bugs/80581), avril 2024). La demande la plus suivie est une issue vers la source brute ([forum 73866](https://forum.obsidian.md/t/add-toggle-for-plaintext-table-editing-in-live-preview-mode/73866)).

Obsidian imbrique un éditeur parce que CodeMirror ne sait pas poser un caret dans un widget. Cerata possède sa mise en page et n'a pas cette contrainte. Son cœur a déjà ce qu'il faut :

- des transactions multi-plages atomiques (`state.rs:41-52`, `:333-367`) ;
- une étape d'annulation par commande (`history.rs:120-125`) ;
- l'IME branché sur le modèle (`editor/input.rs:50-86`) ;
- un crochet de réécriture de la frappe (`input.rs:106`).

Le décorateur traite déjà les nœuds inline des cellules (`decorate.rs:264-268`).

## Décision

1. **Un seul buffer.** Le caret et la sélection d'une cellule sont ceux du document, posés dans la plage source de la cellule. Il n'y a ni éditeur imbriqué ni copie du texte de la cellule.
2. **Le tableau reste dessiné** tant que chaque plage de sélection est contenue dans le contenu d'une cellule du tableau, ou forme une sélection rectangulaire de cellules. La source se révèle, comme aujourd'hui, dans trois cas : une sélection qui franchit le bord du tableau, le tableau entier sélectionné (triple clic), ou la commande « Modifier en Markdown ». Un tableau dont une ligne a plus de cellules que l'en-tête n'est pas édité en place : GFM ne dessine pas ces cellules, donc sa source se révèle au contact, comme dans Writer, pour qu'elles restent visibles pendant l'édition (revue d'interface d'EP-006).
3. **Grille dérivée de l'arbre** (`table::Grid`). Elle porte, en offsets source :
   - le slot et le contenu de chaque cellule ;
   - la position d'insertion d'une cellule vide ;
   - les alignements ;
   - le préfixe de conteneur de chaque ligne.

   Elle remplace le second découpage sur chaîne (`table.rs:56-81`) et le reparse par cellule de `preview::render`.
4. **Positions structurelles interdites au caret.** Après chaque commit, une tête posée sur un `|`, un padding, la ligne délimiteur, un préfixe ou une fin de ligne du tableau va au contenu de cellule voisin, dans le sens du mouvement. C'est une passe sur le modèle d'`atomic::skip` (`state.rs:444-451`), qui connaît la sélection précédente. Les filtres de sélection ne la voient pas et ne tournent que sur une sélection explicite (`state.rs:437`, ADR 0006).
5. **Le texte d'une cellule reste une cellule GFM valide.** Un `|` tapé, collé ou remplacé devient `\|`. Un saut de ligne devient `<br>`, que la cellule rend comme un saut de ligne. Retour arrière au début et Suppr à la fin d'une cellule ne fusionnent jamais deux cellules.
6. **Modifications minimales.** Une frappe ne modifie que la plage de sa cellule. Le padding n'est recalculé que par « Formater le tableau », en largeur d'affichage Unicode. Les autres commandes de structure ne réécrivent que les cellules qu'elles créent, déplacent ou suppriment.
7. **Géométrie calculée par l'éditeur, dans la frame de la frappe.** `layout.rs` met en page chaque cellule comme une ligne courte : texte coupé à la largeur de sa colonne, carte d'affichage tirée du flux de décorations. L'élément éditeur peint la grille. Caret, sélection, hit-test, Haut et Bas, et fenêtre IME (`caret_bounds`, `mod.rs:695`) passent par cette géométrie. Elle remplace l'arbre de `div` (`blocks.rs:427-478`) et la mesure au frame suivant (`element.rs:615-641`).
8. **Clavier**, repris d'Obsidian 1.14.5 sauf mention contraire :
   - Tab et Shift+Tab vont à la cellule suivante ou précédente et sélectionnent son contenu. Tab dans la dernière cellule ajoute une ligne.
   - Entrée va à la cellule du dessous et sélectionne son contenu. Dans la dernière ligne, Entrée ajoute une ligne. Spécifique à Cerata : Entrée dans une dernière ligne entièrement vide la supprime et sort du tableau, comme pour une liste.
   - Shift+Entrée insère `<br>`.
   - Les flèches ne changent de cellule qu'aux bords. Spécifique à Cerata : Haut et Bas gardent la colonne en pixels (Obsidian va au début de la cellule).
   - On sort par le haut ou le bas vers la ligne voisine, créée si le tableau borde le document.
   - Shift+flèche au bord d'un contenu étend une sélection rectangulaire.
9. **Commandes de structure.** Insérer, dupliquer, déplacer et supprimer des lignes et des colonnes, aligner, trier, formater. Chacune est une seule transaction `UserEvent::Command`, donc une étape d'annulation, et reporte les préfixes de citation et de liste.
10. **Presse-papiers.** GPUI ne transporte que du texte et des métadonnées (`gpui/src/platform.rs:3143-3171`). Une sélection rectangulaire se copie donc en tableau GFM, avec la grille en métadonnée JSON. Coller une grille, un tableau GFM ou du TSV remplit les cellules à partir du caret, dans la limite de 10 000 cellules.
11. **Éditeur `source_only`** (panneau Mermaid) : les tableaux y restent du texte brut, comme les blocs atomiques.

## Options écartées

- **Révéler la source au contact** (Writer, état actuel) : c'est précisément le défaut que cette décision corrige.
- **Un éditeur imbriqué par cellule** (modèle d'Obsidian, ou `TextField` de Cerata) :
  - il faut réconcilier deux sélections et deux historiques ;
  - `TextField` n'a ni annulation ni multi-ligne (`text_field.rs:48-67`) ;
  - une écriture hors historique vide l'historique du document (`state.rs:462-464`).

  C'est la classe de bugs d'Obsidian.
- **Normaliser à chaque frappe** (Obsidian 1.5.4 et suivantes) :
  - une frappe modifie toutes les lignes de la colonne, donc les frappes ne se regroupent plus (`history.rs:103-109` exige des changements adjacents) ;
  - les diffs git deviennent bruyants ;
  - la largeur en unités UTF-16 est fausse pour le CJK ;
  - un tableau aligné à la main est réécrit.
- **Garder l'arbre de `div` et récupérer les `TextLayout` après le prepaint** :
  - la hauteur reste mesurée au frame suivant, d'où un saut visible quand une cellule passe à deux lignes ;
  - il faut maintenir une seconde correspondance source vers affichage.
- **Poser le caret dans la cellule en révélant quand même la source** : c'est simple, mais le tableau saute toujours.

## Conséquences

- Écart volontaire avec Writer :
  - `table-decorations.test.ts:134-190` cesse d'être la référence pour la révélation ;
  - `decorate/tests.rs:320-374` et `editor/tests.rs:1433-1493` sont réécrits ;
  - l'oracle différentiel ignore les widgets de tableau (`tests/decorate_oracle.rs:46-49`) ; il désactive aussi les blocs atomiques, sans quoi la passe de la décision 4 déplacerait ses carets aléatoires hors des positions que Writer compare.
- Le PRD change : FR-12 pour les tableaux, et FR-19 à FR-22 ajoutées.
- Le coût d'une frappe dans un tableau comprend trois postes :
  - le reparse du bloc entier (`tree/incremental.rs:63-75`) ;
  - ses décorations, non mises en cache (`state.rs:253-267`) ;
  - la mise en page de ses cellules.
- Le cache de hauteurs des tableaux (`blocks.rs:42`) et les sélecteurs de test `table-cell-{c}` (`blocks.rs:418-426`) disparaissent avec l'arbre de `div`.
- `cerata_markdown` déclare `unicode-width`, déjà dans `Cargo.lock` en dépendance transitive, pour « Formater le tableau ».

## Mesures (US-048, 2026-11-02)

Le spike n'a pas produit de prototype jetable : la mise en page de la décision 7 a été écrite directement dans l'éditeur (`crates/cerata/src/editor/table.rs`) et mesurée par le scénario « frappe dans une cellule » d'`editor_bench` (tableau de 200 lignes et 8 colonnes, 200 frappes dans une cellule de la ligne 100, machine Windows de référence, `docs/benchmarks.md`).

- Première mesure, mise en page synchrone de tout le tableau à chaque frame : p99 35,1 ms. Deux causes : chaque cellule parcourait toutes les décorations et tous les styles du tableau, et le tableau était remis en page à chaque frame, même sans changement.
- Repli retenu : la mise en page du tableau est gardée par état de l'éditeur (révision, sélection, composition, largeur, taille du texte, thème), et chaque cellule trouve ses décorations et ses styles par recherche dichotomique. Après ce repli : p99 4,28 à 4,99 ms (3 × 200 frappes), aucune frappe hors de la frame suivante, gestionnaire d'entrée p99 1,20 à 1,31 ms.
- Repli écarté : la mise en page des seules lignes visibles. Elle n'est pas nécessaire à ce volume, et elle compliquerait Haut, Bas et la hauteur du bloc.

Décision : la mise en page reste synchrone, dans la frame de la frappe. La hauteur d'une cellule qui passe à deux lignes est donc juste dès cette frame.

Précision apportée à la décision 4 : les espaces tapés après le contenu d'une cellule (sa « queue ») sont une position légale pour le caret juste après une modification. Sans cette précision, une espace tapée en fin de cellule, que GFM retire du contenu, enverrait le caret dans la cellule suivante avant le mot suivant. La passe ramène le caret au contenu dès qu'il bouge sans modification.

## Non prouvé

- La fenêtre de candidats IME sous Linux et macOS (US-045, US-046). Sous Windows, c'est prouvé le 2026-11-02 avec MS-IME japonais, piloté par de vraies entrées de l'OS : le texte marqué s'affiche souligné dans la cellule, la liste de prédiction puis la liste de conversion s'ouvrent sous le caret de la cellule, et Entrée valide en place, caret dans la cellule (`docs/captures/ep-006/us-048-ime-*.png`). La position transmise par `bounds_for_range` est aussi testée (`editor/tests.rs`, `typing_and_composing_in_a_cell`).
- L'exposition de la grille aux lecteurs d'écran (rôles AccessKit `Table`, `Row`, `Cell`) dépend de la façon dont US-044 exposera le texte de l'éditeur.
- Une cellule de plusieurs lignes dans une colonne centrée ou alignée à droite est dessinée alignée à gauche : seules les cellules d'une ligne suivent l'alignement de leur colonne.
