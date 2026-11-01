# Captures et revue d'interface d'EP-005 (Windows)

## Revue `/better-interface` des blocs riches (2026-11-01)

Périmètre : les surfaces livrées par EP-005 dans l'éditeur. Il comprend la complétion `[[`, le panneau des propriétés (frontmatter), les blocs HTML (`<details>`, tableau), les formules (en ligne, en bloc, en erreur) et le canvas Mermaid (barre d'outils, panneau source, plein écran, erreur). Il comprend aussi le rail des sections et son sommaire. Thème sombre par défaut, fenêtre de 1216 x 808 px.

`cerata.exe` (build debug) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`) sur un vault de démonstration (`Rich blocks.md`, `Roadmap.md`). Les contrastes sont mesurés sur les pixels capturés. Les fichiers de `%APPDATA%\cerata` ont été sauvegardés avant la session et restaurés à l'identique après (vérifié par `diff -rq`).

Pile : GPUI épinglé par rev. Les jetons de couleur sont dans `crates/cerata/src/editor/theme.rs` et les contrôles clavier partagés dans `crates/cerata/src/a11y.rs`. Il n'existe pas de documentation d'interface propre au projet en dehors des sources de Writer citées par le PRD. Les six skills de domaine ont été chargés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition.

| Domaine | Inspecté | Résultat |
|---|---|---|
| Accessibilité | rôles et noms de chaque contrôle des six fichiers, parcours au clavier dans l'app, tests GPUI | 5 constats, corrigés |
| Mise en page | captures de chaque surface ; hauteur des blocs après ouverture d'un `<details>` | 2 constats, corrigés |
| Rédaction | chaînes de `complete.rs`, `frontmatter.rs`, `html.rs`, `math.rs`, `mermaid.rs`, `rail.rs`, notice Ctrl+M | Clear |
| Typographie | labels des diagrammes, panneau source, tailles du sommaire et de la complétion | 1 constat, corrigé |
| Couleur | contrastes mesurés sur pixels (voir Vérification) | Clear |
| Finition | animation du sommaire, anneaux de focus, surfaces | 1 constat, corrigé |

### Constats corrigés

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| HIGH | Accessibilité | `crates/cerata/src/a11y.rs:15`, `crates/cerata/src/editor/actions.rs:323` | `KeyBinding::new("tab", FocusNext, None)` : une liaison sans contexte a la profondeur maximale dans le keymap de GPUI (`keymap.rs:274-279`), donc Tab déplaçait le focus partout et n'indentait plus dans l'éditeur. Corollaire : rien dans le texte n'était atteignable au clavier | Liaisons de la coquille sous `!Editor`. Dans l'éditeur, Tab indente ; Ctrl+M (Maj+Option+M sur macOS) bascule Tab vers le déplacement du focus, comme `toggleTabFocusMode` de CodeMirror (codemirror.net/docs/ref). Une notice annonce le mode (`role(Status)`) | WCAG 2.1.1 et 2.1.2 : rail, canvas et propriétés atteignables au clavier, sortie du texte possible. Régression d'EP-004 sur l'indentation |
| HIGH | Accessibilité | `crates/cerata/src/editor/actions.rs:239` | Canvas focalisé : Suppr, Retour arrière et Tab modifiaient le texte caché derrière (test : `"\tntro…"`) | Les actions d'édition ne s'appliquent que si l'éditeur (ou sa barre de recherche) a le focus (`keys_reach_text`, `mod.rs`) | Perte de données silencieuse |
| HIGH | Accessibilité | `crates/cerata/src/editor/rail.rs:132`, `crates/cerata/src/editor/mermaid.rs:424` | Repères du rail et boutons du canvas : `role(Button)` sans être focusables ; « Copy heading link » seulement au clic droit | `a11y::button` : arrêt de tabulation, anneau visible, Entrée et Espace. Sur un repère, Shift+F10 ouvre le menu, puis Entrée le lance et Échap le ferme. Entrée sur un bouton du canvas le clique sans basculer aussi le panneau | WCAG 2.1.1 |
| HIGH | Accessibilité | `crates/cerata/src/editor/complete.rs:205` | Lignes cliquables sans rôle ni nom ; la sélection n'était pas exposée | `ListBox` « Notes », options `ListBoxOption` nommées « note, dossier » avec `aria_selected`, comme la palette | WCAG 4.1.2 |
| MEDIUM | Accessibilité | `crates/cerata/src/editor/mod.rs` (`reveal_focused_block`) | Le focus clavier pouvait tomber sur un bouton du canvas hors de la fenêtre (« − » invisible) | Un bloc qui reçoit le focus défile une fois en haut de la zone sûre (`revue-clavier-barre-canvas.png`) | WCAG 2.4.11 |
| MEDIUM | Accessibilité | `crates/cerata/src/editor/frontmatter.rs:477` | Tab au dernier champ des propriétés restait sur place | Au-delà du premier ou du dernier champ, Tab sort du panneau | WCAG 2.1.2 |
| MEDIUM | Mise en page | `crates/cerata/src/editor/element.rs:618` | Ouvrir un `<details>` dessinait son contenu sur la ligne suivante : le `cx.notify()` de la re-mesure, émis pendant le dessin, est ignoré par GPUI (`window.rs:175`) | Re-mesure demandée via `window.on_next_frame` ; le bloc suivant descend (`us-035-details-ouvert.png`) | Contenu masqué par chevauchement |
| MEDIUM | Mise en page | `crates/cerata/src/editor/mermaid.rs:338`, `crates/cerata/src/editor/mod.rs` (`source_only`) | Panneau source à 33 % : l'éditeur imbriqué gardait les marges de page et rendait la fence en bloc de code étroit à défilement horizontal, avec des lignes tronquées | Mode source : texte brut sur toute la largeur, 12 px, interligne 1,5 (`mermaid-canvas.css:115-121`) | Contenu tronqué |
| MEDIUM | Typographie | `crates/cerata_render/src/js.rs` (`root_font`), `mermaid.rs:219` | Libellés des diagrammes en Times New Roman : resvg n'applique pas le `style` racine de Pele | Polices UI et mono passées à Pele (`diagramFonts`, `mermaid-renderer.ts:34-41`) et répétées en attribut `font-family` | Police hors système |
| MEDIUM | Accessibilité | `crates/cerata/src/editor/complete.rs:144` | Avec 20 notes, la liste (280 px max.) ne suivait pas la sélection au clavier | `ScrollHandle::scroll_to_item` | Sélection invisible |
| LOW | Finition | `crates/cerata/src/editor/rail.rs:250` | Fondu de 180 ms du sommaire même avec `reduce_motion` | Ouverture sans animation sous `reduce_motion`, comme le reste de l'app | Cohérence |
| LOW | Accessibilité | `crates/cerata/src/editor/rail.rs:205`, `frontmatter.rs:525` | Lignes du sommaire et « Add property » sans `aria_label` | Nom explicite | Convention du projet (palette, réglages) |

Corrigé au passage : une note qui s'ouvre avec un frontmatter place le caret dans le corps, et les propriétés s'affichent donc en panneau, comme dans Writer qui sépare le frontmatter du texte (`editor/mod.rs`, test `a_note_opens_with_its_properties_as_a_panel`).

### Constats laissés ouverts (LOW)

Le menu contextuel de l'éditeur (EP-003) n'a pas de navigation clavier propre ; seul le menu d'un repère du rail est piloté au clavier. La notice Ctrl+M reste affichée jusqu'à la prochaine notice et masque le bas de la zone de texte. Le bouton plein écran du canvas utilise une icône de carré. Le plein écran a une bordure d'accent quand il a le focus.

### Vérification

Tests GPUI (chemin réel, entrées clavier et souris) :

| Test | Ce qu'il vérifie |
|---|---|
| `the_rail_works_from_the_keyboard` (`shell/tests.rs`) | Ctrl+M puis Tab, Entrée saute au titre, Shift+F10 puis Entrée copie le lien, Échap ferme, Ctrl+M rétablit l'indentation |
| `the_toolbar_works_from_the_keyboard` | Tab depuis le canvas atteint la barre, Entrée clique une seule fois |
| `a_focused_canvas_scrolls_into_sight` | Le bloc focalisé est ramené à l'écran |
| `tab_leaves_the_panel_past_its_last_field` | Tab sort du panneau des propriétés |
| `the_canvas_pans_zooms_and_fits` | Suppr, Tab et Maj+Tab ne touchent pas le texte |
| `wiki_completion_lists_twenty_notes_and_absorbs_the_brackets` | La liste défile jusqu'à la vingtième note |
| `html_blocks_render_toggle_and_reveal` | Le caret suivant est peint sous le corps ouvert, après livraison du frame suivant |

App réelle : parcours Ctrl+M puis Tab x 10, avec l'anneau sur « Edit code », sur un repère du rail et sur « + » ramené à l'écran (`revue-clavier-*.png`).

Contrastes mesurés sur pixels, thème sombre :

| Paire | Mesure | Seuil |
|---|---|---|
| Repère inactif du rail (35 %) sur le fond | 3,12:1 | 3:1 (graphique) |
| Repère actif | 18,55:1 | 3:1 |
| Ligne du sommaire (muted) sur le panneau | 8,31:1 | 4,5:1 |
| Ligne active (accent) | 6,00:1 | 4,5:1 |

Non vérifié :

- Lecture par un lecteur d'écran (Narrateur, NVDA).
- Thème clair.
- Zoom système à 200 %.
- Mode Contraste élevé.
- macOS et Linux.

Verdict : Approve. Aucun constat HIGH ne subsiste dans le périmètre. Les six domaines ont été inspectés.

## Revue `/better-interface` des tableaux, des bornes de diagramme, des embeds et du rail long (2026-11-01)

Périmètre : les surfaces d'EP-005 qui n'avaient pas encore été revues. Il comprend les tableaux GFM rendus en place (US-032 : cellules en Markdown, alignements, colonnes plafonnées à 48 caractères, source révélée au contact) et le caret avant et après un diagramme Mermaid (ADR 0006, blocs atomiques). Il comprend aussi les embeds d'image Obsidian `![[pic.png]]` (US-033), résolus et introuvables, et le rail des sections sur une note de 481 titres (repères fenêtrés, sommaire virtualisé). Thème sombre, fenêtre de 1216 x 808 px, zoom de l'éditeur à 100 % et 200 %.

Même protocole que la revue précédente : `cerata.exe` (build debug) piloté par de vraies entrées de l'OS sur un vault de démonstration (`Rich blocks.md` avec tableau, diagramme et embeds, `assets/pic.png`, `Long note.md` de 400 sections). La note est ouverte seule, puis dans son vault. Les contrastes sont mesurés sur les pixels capturés. `%APPDATA%\cerata` a été sauvegardé avant la session et restauré à l'identique après (`diff -rq`). Les six skills de domaine ont été chargés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition.

| Domaine | Inspecté | Résultat |
|---|---|---|
| Accessibilité | rôles et noms des images, du canvas et du sommaire ; chemin clavier aux bornes du diagramme (Bas, Haut, frappe, Retour arrière) | 1 constat, corrigé |
| Mise en page | tableau à 100 % et 200 % ; embed résolu, introuvable, en ligne ; sommaire en fin de note | 1 constat ouvert (MEDIUM) |
| Rédaction | source montrée par un embed introuvable, noms accessibles | Clear |
| Typographie | cellules (gras, italique, code, lien), colonne longue repliée à droite, source révélée en mono | Clear |
| Couleur | jeton des surfaces flottantes ; contrastes mesurés (voir Vérification) | 1 constat, corrigé |
| Finition | caret de bord sur le canvas, sélection du bloc entier | 1 constat ouvert (LOW) |

### Constats corrigés

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| HIGH | Couleur | `crates/cerata/src/editor/rail.rs:326`, `crates/cerata/src/editor/complete.rs:221`, `crates/cerata/src/editor/mod.rs:889` | Le sommaire, la liste de complétion `[[` et la notice de l'éditeur prenaient le fond `panel` (`surface_subtle`), translucide en thème sombre. Sur la note longue, le texte des paragraphes traversait les lignes du sommaire (« Section 387 » sous « Paragraph for section ») | Fond `overlay`, que `theme.rs:23-25` réserve à « what floats over text » et qu'utilisent déjà les menus, la palette et la recherche (`rail-note-longue.png`) | Texte illisible ; jeton employé hors de son rôle |
| MEDIUM | Accessibilité | `crates/cerata/src/editor/images.rs:552`, `:644` | Images Markdown, HTML et embeds dessinées sans rôle ni nom, alors que le canvas Mermaid est une `Image` nommée | `role(Image)` et nom du fichier, comme un navigateur nomme un `<img>` sans alt (test `images_are_named_by_their_file`) | WCAG 1.1.1 |

Corrigé au passage, révélé par la suite complète : le panneau source d'un diagramme relançait son délai de 150 ms à chaque repeinte de son éditeur. Depuis que le rail relit les titres 150 ms après la dernière frappe et repeint, la deuxième modification du panneau pouvait ne jamais atteindre la note (`the_edit_panel_rewrites_the_fence_after_150_ms`, déterministe). Le délai ne repart plus que sur une nouvelle révision du panneau (`mermaid.rs:393`). Le rail ne repeint que si les titres ont changé (`rail.rs:102`).

### Constats laissés ouverts

| Sévérité | Domaine | Emplacement | Constat |
|---|---|---|---|
| MEDIUM | Mise en page | `crates/cerata/src/editor/mod.rs:684` | Au premier Ctrl+Fin sur une note, une image chargée après le défilement agrandit sa ligne et pousse le caret sous la barre d'état. Une seconde touche le ramène. Même comportement pour `![](…)` depuis EP-003, et CodeMirror ne redéfile pas non plus au chargement d'une image |
| LOW | Finition | `crates/cerata/src/editor/element.rs:354` | Le caret aux bornes d'un diagramme recouvre le bord du canvas (1 px) et dépasse de sa marge verticale (`adr-0006-caret-*.png`) |
| LOW | Mise en page | `crates/cerata/src/editor/layout.rs:476` | Un embed au milieu d'une ligne garde sa source, en couleur de lien, comme les images Markdown en ligne depuis EP-003 ; Writer l'affiche en ligne |

Conformes à Writer et donc non retenus : un clic sur un tableau sélectionne toute sa source (`fold/core.ts:170-205`, enregistré pour `cm-table-widget`), et le titre actif suit le haut de la vue, pas le caret (`use-active-headings.ts`). Un embed introuvable montre sa source atténuée (`.cm-image-embed-unresolved`). Hors d'un vault, `![[pic.png]]` ne cherche que dans le dossier de la note.

### Vérification

App réelle :

| Parcours | Résultat |
|---|---|
| Fin de « Text between… », Bas, Bas, Haut | Caret au début du bloc, puis à sa fin, puis au début, dessiné sur toute la hauteur du canvas (`adr-0006-caret-avant.png`, `adr-0006-caret-apres.png`) |
| Frappe « Above » au début du bloc, Ctrl+Z | « Above » sur sa propre ligne au-dessus du diagramme, annulé d'un coup |
| Retour arrière à la fin du bloc | Le bloc entier est sélectionné, rien n'est supprimé |
| Note seule, puis vault | `![[pic.png]]` introuvable puis résolu depuis `assets/` (`embed-introuvable.png`, `embed-vault.png`) |
| Zoom 200 % | Le tableau tient dans la colonne, les cellules se replient, rien n'est rogné (`us-032-zoom-200.png`) |
| Clic sur le tableau | Source révélée en mono, sélectionnée entière (`us-032-source.png`) |
| Ctrl+Fin sur `Long note.md`, survol du rail | Sommaire opaque, centré sur « Section 399 », titre actif en accent (`rail-note-longue.png`) |

Contrastes mesurés sur pixels, thème sombre :

| Paire | Mesure | Seuil |
|---|---|---|
| Ligne du sommaire (muted) sur `overlay` | 8,71:1 | 4,5:1 |
| Ligne active (accent) sur `overlay` | 6,58:1 | 4,5:1 |
| En-tête de tableau sur son fond | 16,43:1 | 4,5:1 |
| Cellule sur le fond de page | 18,41:1 | 4,5:1 |
| Source d'un embed introuvable (muted) | 8,71:1 | 4,5:1 |

Les bordures du tableau (1,22:1) et du sommaire (1,20:1) structurent sans porter d'information propre : 1.4.11 ne s'y applique pas.

Commandes, sur l'état final : `cargo fmt --all --check` OK, `cargo clippy --workspace --all-targets -- -D warnings` OK, `cargo test --workspace` 728 réussis, 0 échec. Le module `mermaid::` a été relancé 5 fois sans échec.

Non vérifié :

- Lecture par un lecteur d'écran (Narrateur, NVDA), dont l'annonce du caret aux bornes d'un diagramme (déjà noté dans l'ADR 0006).
- Thème clair.
- Zoom système de Windows à 200 % (seul le zoom de l'éditeur a été testé).
- Mode Contraste élevé.
- macOS et Linux.

Verdict : Approve. Aucun constat HIGH ne subsiste dans le périmètre. Les six domaines ont été inspectés.

## Captures

| Fichier | Contenu |
|---|---|
| `us-031-*.png` | Spike des moteurs (ADR 0005) |
| `us-033-completion-wiki.png`, `us-033-lien-accepte.png` | Complétion `[[` dans un vault, puis le lien inséré |
| `us-034-us-036-proprietes-et-formules.png` | Panneau des propriétés (valeur complexe en lecture seule), formule en ligne, formule en bloc et formule en erreur |
| `us-035-details-ferme.png`, `us-035-details-ouvert.png` | `<details>` et tableau HTML ; à l'ouverture, le bloc suivant descend |
| `us-037-diagramme.png`, `us-037-panneau-source.png`, `us-037-plein-ecran.png` | Canvas ajusté, panneau source à 33 %, plein écran |
| `us-038-sommaire.png`, `us-038-saut-vers-titre.png` | Sommaire au survol (titre actif en accent), puis saut vers « Tables » |
| `revue-clavier-*.png` | Parcours au clavier après Ctrl+M |
| `us-032-tableau.png`, `us-032-source.png`, `us-032-zoom-200.png` | Tableau GFM rendu (gras, code, lien, alignements, cellule longue repliée), sa source révélée au clic, le même à 200 % |
| `adr-0006-caret-avant.png`, `adr-0006-caret-apres.png` | Caret au début et à la fin d'un diagramme atomique |
| `embed-introuvable.png`, `embed-vault.png` | `![[pic.png]]` hors vault (source atténuée), puis résolu dans le vault |
| `rail-note-longue.png` | Sommaire d'une note de 481 titres, opaque, centré sur le titre actif |
