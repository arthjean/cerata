# ADR 0006 : blocs atomiques, le diagramme reste dessiné et le caret l'enjambe

> Statut : **décidé** le 2026-11-01. Story : US-037. Référence : `mermaid-decorations.ts:186-197`, `revealBlockOnArrow.ts:62-82`, `SPECs/mermaid-drag-selection-edit-mode-flip-spec.md` de writer-computer ; tests `mermaid.test.ts:501-511`.

## Contexte

Dans Writer, une fence ` ```mermaid ` fermée est toujours remplacée par son canvas, même quand la sélection la touche. On édite le diagramme dans le panneau du canvas, pas en révélant la source. La première version de Cerata révélait la source dès que le caret touchait la fence, comme pour les tableaux et les formules. Résultat : 30 lignes de source apparaissent quand le caret frôle le diagramme, la mise en page saute, et le panneau d'édition devient redondant. C'est contraire aux deux invariants de `mermaid.test.ts`.

Garder le canvas quand la sélection touche la fence pose une question que le moteur ne tranchait pas : que faire d'un caret qui tombe à l'intérieur d'une source cachée ? Les filtres de sélection existants (`SelectionFilter`) ne voient pas la sélection précédente et ne peuvent donc pas choisir un sens.

## Décision

Une fence mermaid fermée et non vide est un **bloc atomique** (`cerata_markdown::atomic`), au sens des `atomicRanges` de CodeMirror :

- `decorate` la remplace par son canvas quelle que soit la sélection.
- `EditorState::commit` applique `atomic::skip` à chaque sélection : une tête strictement à l'intérieur va à la borne du côté d'où elle vient. Une ancre à l'intérieur va à la borne qui garde le bloc entier sélectionné. Les deux bornes, début et fin de la fence, sont des positions valides.
- Haut et Bas : du début du bloc vers sa fin, de sa fin vers son début, de sa fin vers la ligne suivante (`atomic::step_over`). Depuis une ligne voisine séparée par des lignes vides, le comportement de `revealBlockOnArrow` mène à la borne.
- Une suppression qui entamerait le bloc (Retour arrière à sa fin, Suppr à son début, suppressions de mot) sélectionne le bloc. La suivante le supprime entier. CodeMirror supprime directement la plage atomique ; Cerata demande une confirmation par la sélection parce qu'un diagramme représente beaucoup de texte.
- Du texte tapé sur une borne va sur sa propre ligne, au-dessus ou au-dessous du bloc, au lieu de casser la fence (`atomic::typed_at`). Entrée insère un saut de ligne normal.
- Le caret au début du bloc se dessine sur son bord gauche, à la fin sur son bord droit, de la hauteur du canvas. Les lignes cachées de la source ne dessinent pas de caret.
- L'éditeur du panneau source (`Editor::source_only`) désactive le mécanisme (`EditorState::set_atomic_blocks(false)`) : il affiche la fence en clair.

## Options écartées

- **Révéler la source au contact** (première version) : contraire à Writer et à ses tests.
- **Interdire les bornes et sauter directement aux lignes voisines** : un diagramme en tête ou en fin de document serait inatteignable, et on ne pourrait plus taper juste avant ou juste après lui.
- **Un filtre de sélection** : ne connaît pas la sélection précédente, donc pas le sens du mouvement.

## Conséquences

- Seuls les diagrammes sont atomiques. Tableaux, formules et HTML gardent la révélation de la source au contact, comme dans Writer.
- Une occurrence de recherche située dans la source d'un diagramme ne peut pas être sélectionnée : la sélection est repoussée à une borne. Writer ne l'affiche pas davantage, la source étant remplacée.
- Le coût par transaction est une résolution d'arbre par plage de sélection.

## Non prouvé

- Le comportement sous un lecteur d'écran : le canvas est atteignable au clavier par le mode de focus (Ctrl+M), mais la position du caret aux bornes n'est pas annoncée.
