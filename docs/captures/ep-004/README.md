# Captures et revue d'interface d'EP-004 (Windows)

Revue `/better-interface` de la coquille de l'application (EP-004), le 2026-11-01, sur la machine de référence Windows (`docs/benchmarks.md`). `cerata.exe` est piloté par de vraies entrées de l'OS (souris, clavier et molette par `SendInput`, disposition AZERTY), chaque étape capturée dans la zone cliente. Les contrastes sont mesurés sur les pixels capturés, fond réellement rendu compris (la fenêtre est translucide).

Ce rapport est joint à la revue des stories US-023 à US-030.

## Périmètre

Fenêtres et session, onglets et navigation, sidebar (sections, arbre, menus, renommage, suppression), bandeau de conflit, palette de commandes, accueil et nouvel onglet, fenêtre compacte et son sélecteur de récents, panneau de réglages, thèmes clair et sombre, barre d'état, menus dessinés (barre de menus, menus contextuels de la coquille), notice de fenêtre. États vides, d'erreur, de chargement, fenêtre étroite (800 px) et zoom à 200 % compris.

Hors périmètre : la surface de l'éditeur (EP-003), dont seuls le fond du menu contextuel et de la barre de recherche ont été corrigés ici parce qu'ils partageaient le défaut de translucidité.

Domaines passés, dans l'ordre : accessibilité, mise en page, rédaction, typographie, couleur, finition. Aucune documentation d'interface propre au projet en dehors des spécifications de Writer citées par le PRD.

## Constats corrigés

| Sévérité | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|
| HIGH | `crates/cerata/src/editor/theme.rs:108`, `crates/cerata/src/theme.rs:272` ; palette, menus, barre de menus, sélecteur compact, notice, barre de recherche | Surfaces flottantes peintes avec le fond translucide de la fenêtre : le texte de la note se lit à travers (`avant-palette-translucide.png`) | Jeton `overlay` opaque (`bg_base`) sur toutes les surfaces flottantes (`apres-palette-opaque.png`) | Texte superposé illisible (contraste du texte de contrôle) |
| HIGH | `crates/cerata/src/theme.rs:216` | Texte secondaire à 3,96:1 (clair), 4,49:1 (sombre), 4,02:1 sur l'accueil, 4,16:1 dans la barre d'état | `muted` mélangé à 68 % : 7,52:1 mesuré ; un test vérifie 4,5:1 sur chaque préréglage, chaque mode, fenêtre et ligne sélectionnée | WCAG 1.4.3 |
| HIGH | `crates/cerata/src/editor/theme.rs:105`, `crates/cerata/src/theme.rs` | Texte d'erreur à 2,00:1 | `danger` propre au mode (#a40e26 clair, #ffb3ab sombre) : 9,24:1 mesuré, couvert par le même test | WCAG 1.4.3, erreurs illisibles |
| HIGH | `crates/cerata/src/menu.rs:174`, `:239` ; `crates/cerata/src/shell/menu_bar.rs` | Menus de la coquille sans clavier ; Échap ne fermait pas, la barre de menus dessinée restait ouverte | Le menu prend le focus, flèches (séparateurs et entrées désactivées sautés), Droite et Gauche pour les sous-menus, Entrée et Espace, Échap rend le focus (`apres-menu-clavier.png`) | WCAG 2.1.1 |
| HIGH | `crates/cerata/src/a11y.rs:27` ; onglets, sidebar, réglages, accueil, nouvel onglet, bandeau, sélecteur compact, palette | Contrôles atteignables à la souris seulement, sans nom accessible ni anneau de focus | Arrêt de tabulation, rôle AccessKit, nom, état (`aria_selected`, `aria_expanded`, `aria_toggled`, valeur des curseurs), anneau visible au clavier seulement ; Tab et Maj+Tab dans la coquille ; curseurs aux flèches, Origine et Fin ; arbre : touche Menu ou Maj+F10, F2, Suppr | WCAG 2.1.1, 2.4.7, 4.1.2 |
| HIGH | `crates/cerata/src/shell/mod.rs` (`on_save_result`) | Un échec d'enregistrement n'était signalé que par un point rouge sur l'onglet | Notice une fois par série d'échecs : « Cannot save {name}: {error}. Your changes stay in the editor; the next edit tries again. » ; le point porte une description accessible | Couleur seule (1.4.1), erreur sans issue |
| HIGH | `crates/cerata/src/shell/status_bar.rs` | Barre d'état posée par-dessus le texte : à 200 % elle recouvre la dernière ligne (`avant-zoom-barre-etat.png`) | Barre d'état dans le flux, sous le contenu (`apres-zoom.png`) | WCAG 1.4.4, contenu masqué |
| HIGH | `crates/cerata/src/shell/mod.rs:131` | Sous AZERTY, Ctrl+0 et Ctrl+1 à 9 n'arrivaient jamais (Windows nomme la touche par son caractère : `ctrl-à`) | Liaisons supplémentaires par position de touche (chiffres, `=`, `\`, `,`) ; Ctrl+0 vérifié sous AZERTY | Raccourcis documentés inopérants |
| MEDIUM | `crates/cerata/src/editor/text_field.rs:82` | Champs des réglages à 220 px fixes, qui débordent de leur carte ; sélection peinte sans focus (`avant-reglages.png`) | Largeur du conteneur, sélection seulement avec le focus (`apres-reglages.png`) | Mise en page, état trompeur |
| MEDIUM | `crates/cerata/src/shell/compact.rs:262` | Chemins des récents tronqués à la fin : tous identiques | Dossier personnel en `~`, puis seulement les derniers composants après `…` (`apres-recents-compacts.png`) | Contenu tronqué sans moyen d'y accéder |
| MEDIUM | `crates/cerata/src/settings_panel.rs:88` | Catégorie « Privacy » (télémétrie) affichée alors que la v1 n'en envoie aucune | Catégorie masquée | Réglage trompeur |
| LOW | `crates/cerata_core/assets/settings.schema.json:84` | « Schema », « Light or dark color schema », descriptions de contraste et mention de Writer | « Color Scheme », « Light, dark, or the system setting », descriptions corrigées, « the platform default » | Rédaction |

## Constats laissés ouverts (LOW)

La capitalisation « UI font » vient du schéma de Writer, porté tel quel. Le dialogue de suppression ne nomme pas la Corbeille et écrit « file(s) ». Les titres longs sont tronqués dans le sélecteur compact (le chemin complet reste dans le nom accessible). Les lignes active et sélectionnée de la sidebar se ressemblent, comme dans Writer. Les raccourcis de l'éditeur sous AZERTY et l'accessibilité des surfaces d'EP-003 relèvent d'EP-003.

## Performance de l'arbre (US-026)

`us-026-arbre-50k.png` : 50 000 notes dans un seul dossier, arbre affiché par nom puis complété par les titres, après 300 crans de molette. Valeurs et procédure dans `docs/benchmarks.md`.

## Vérification

Chaque correctif a été revu sur l'application lancée (captures `apres-*`), en clair et en sombre, à 800 px de large et à 200 % de zoom. Tests de régression : contraste des jetons sur chaque préréglage, clavier des menus et restitution du focus, notice d'échec d'enregistrement, chemins des récents, liaisons par position, arbre de 50 000 fichiers.

Non vérifié : lecture par un lecteur d'écran (Narrateur ou NVDA) de l'arbre AccessKit, mode Contraste élevé de Windows, macOS et Linux (EP-007, EP-008).

Verdict : Approve. Aucun constat HIGH ne subsiste dans le périmètre.
