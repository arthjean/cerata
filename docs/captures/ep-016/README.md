# Captures d'EP-016 (Windows)

## Vérification dans l'app (2026-11-03)

`cerata.exe` (build debug de l'epic) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`, sélecteurs de dossier et de fichier de Windows). Pour simuler un premier lancement, le dossier de données de l'app est mis de côté (sauvegarde vérifiée avec `diff -r`), puis restauré à l'identique après le run. Les dossiers ouverts sont des dossiers jetables : une copie de `crates/cerata/examples/fixtures/` (`Fixtures`), un dossier `Sync` qui contient `.stfolder`, un dossier `loose` avec une note isolée, et un `home` qui reçoit la bibliothèque créée. Fenêtre de 1216 × 808 px, thème sombre. Le disque et `libraries.json` sont relus après chaque geste.

| Story | Geste | Entrée réelle | Résultat | Capture |
|---|---|---|---|---|
| US-078 | Premier lancement | lancement sans dossier de données | écran « Welcome to Cerata » : formulaire « Create Library », nom préréglé à « Cerata », emplacement par défaut, « Open Folder… », « Open File… » | `us078-accueil.png` |
| US-078 | Emplacement synchronisé | « Change… », dossier `Sync` choisi dans le sélecteur | avertissement « This folder is synced by Syncthing. Keep it always available offline, and don't sync it with a second service. » ; « Create » reste actif | `us078-synchro.png` |
| US-078 | Créer | « Change… » vers `home`, clic sur « Create » | `home\Cerata\.cerata\id` (33 octets : 32 caractères hexadécimaux et un saut de ligne) ; `libraries.json` la marque par défaut ; la bibliothèque s'ouvre dans la fenêtre | `us078-creee.png` |
| US-078 | Liste | « Close Workspace » | l'accueil liste la bibliothèque sous « Libraries » | `us078-liste.png` |
| US-080 | Sélecteur | « Open Folder… » sur `Fixtures`, Ctrl+P, « switch », Entrée | « Switch Library… » liste les deux bibliothèques, avec chemin et « Default » | `us080-selecteur.png` |
| US-080 | Actions | Tab sur une ligne | « Open », « Set as Default », « Remove from List » ; Escape revient à la liste | `us080-actions.png` |
| US-080 | Bibliothèque déplacée | `home\Cerata` renommé en `Cerata-moved` hors de l'app, puis le sélecteur rouvert | la ligne affiche « Not found » ; la choisir propose « Locate… » et « Remove from List » au lieu d'ouvrir | `us080-introuvable.png` |
| US-077 | Relance | app fermée puis relancée | `Fixtures` est restaurée avec ses onglets | (pas de capture) |
| US-078 | Ligne introuvable | « Close Workspace » | l'accueil marque Cerata « Not found », avec « Locate… » et « Remove from List » | `us078-introuvable.png` |
| US-079 | Ctrl+N, défaut introuvable | Ctrl+N sur l'accueil | aucun fichier créé ; notice « Your default library wasn't found. Locate it or create a new one. » avec ses deux boutons | `us079-defaut-introuvable.png` |
| US-080 | Localiser | « Locate… » dans la notice, `Cerata-moved` choisi | la bibliothèque s'ouvre ; `libraries.json` garde le même id avec le nouveau chemin | `us080-localisee.png` |
| US-079 | Fichier isolé | « Close Workspace », « Open File… » sur `loose\note.md` | fenêtre compacte sur la note ; aucun `.cerata` dans `loose` ; le registre garde deux entrées | `us079-fenetre-compacte.png` |

**Défaut trouvé et corrigé pendant le run.** Après un Escape dans la palette sur une page sans éditeur (accueil, onglet vide), Ctrl+P ne faisait plus rien : `close_palette` ne rendait le focus qu'à un éditeur, et la fenêtre restait sans focus. `close_palette` rend maintenant le focus à la page (`focus_page`). Régression : `dismissing_the_palette_on_a_new_tab_keeps_the_shortcuts` (`crates/cerata/src/shell/tests.rs`), en échec sans le correctif.

## Migration sur une copie des données réelles

`cargo run -p cerata_core --example migration_check -- <copie du dossier de données>`, sur une copie, jamais sur le dossier live :

```
before: 6 recent workspaces, 4 session keys, 0 pin keys
registry: 6 libraries (0 recent folders gone, skipped)
```

Les six espaces récents deviennent six bibliothèques. Les sessions passent sous l'id de leur bibliothèque, avec des chemins relatifs, sans perte : 2 → 2, 2 → 2, 1 → 1, 1 → 1, 0, 0 onglets, et tous les fichiers des onglets sont présents. Aucun marqueur n'est écrit dans les dossiers réels (l'exemple n'ouvre pas les bibliothèques), et le dossier live reste intact.

## Revue d'interface (`/better-interface`)

**Périmètre.** Écran d'accueil (formulaire « Create Library », avertissement de synchronisation, problème de création, liste avec lignes « Not found »), notice de fenêtre avec boutons d'action, palette « Switch Library… » et son niveau d'actions. Code : `crates/cerata/src/shell/pages.rs` (`welcome`, `library_row`, `notice`), `crates/cerata/src/shell/libraries.rs`, `crates/cerata/src/palette.rs`. Conventions lues : `AGENTS.md`, `docs/captures/ep-007/` et `ep-015/`. Les six skills de domaine sont chargés : accessibilité, mise en page, rédaction, typographie, couleurs, UI.

**Contraste mesuré sur les captures (thème sombre).** « Not found » 10,59:1 ; chemins atténués 8,46:1 ; sous-titre et avertissement environ 8,5:1. Tout passe.

| Sévérité | Constat | Emplacement | Suite |
|---|---|---|---|
| HIGH | Anneau de focus invisible sur le bouton primaire « Create » : la bordure de focus prend la couleur du texte, qui est aussi le fond du bouton | `pages.rs`, `welcome_button` | corrigé : au focus, le bouton s'inverse (fond de la page, texte et bordure couleur texte) |
| HIGH | Chemins tronqués sans accès à la valeur complète (emplacement du formulaire, chemin des lignes de bibliothèque) | `pages.rs`, `welcome` et `library_row` | corrigé : troncature retirée, le chemin passe à la ligne |
| HIGH | « Couldn't create the library: {error} » ne dit pas comment s'en sortir | `libraries.rs`, `WelcomeProblem::message` | corrigé : « … Choose another location or name. » ; le test vérifie la phrase |
| MEDIUM | « Set as Default » et « Remove from List » ne s'atteignent dans la palette que par Tab, pas au pointeur | `palette.rs` | consigné, non corrigé : l'accueil offre les mêmes actions au pointeur |
| MEDIUM | Deux vocabulaires côte à côte : « Open Workspace », « Close Workspace » et « Switch Library… », « Open Folder… » | `palette.rs` | consigné, non corrigé : renommer les commandes existantes sort de l'epic |
| LOW | Placeholder « Find a library... » en points ASCII | `palette.rs` | convention du projet, gardée |
| LOW | Le problème de création s'affiche avec son texte, sans région de statut annoncée | `pages.rs` | consigné |
| LOW | « Create » inactif plutôt qu'une validation à l'envoi | `pages.rs` | imposé par le critère du PRD |

**Non vérifié.** Les trois correctifs HIGH sont prouvés par le code et les tests, pas par une nouvelle capture dans l'app réelle : les captures ci-dessus précèdent les correctifs. Le thème clair, le lecteur d'écran et le zoom à 200 % ne sont pas vérifiés.

**Verdict.** Approuvé une fois les trois HIGH corrigés, sous réserve de la recapture du focus de « Create » et d'un chemin long qui passe à la ligne.

## Revue de l'epic (2026-11-03)

`cerata.exe` (build debug après les correctifs ci-dessous), même protocole : dossier de données mis de côté puis restauré, identique à sa sauvegarde (`diff -r`) après chaque run. Entrées réelles : `SetCursorPos`, `mouse_event` (clic, molette), `SendKeys` (Tab), envoyées seulement si `cerata.exe` est au premier plan. Le registre de test contient des bibliothèques absentes du disque, à chemin long.

| Geste | Entrée réelle | Résultat | Capture |
|---|---|---|---|
| Focus de « Create » | clic dans Name, puis Tab, Tab | le bouton s'inverse (fond de la page, bordure et texte couleur texte) : le focus est visible | `us078-focus-create.png` |
| Chemin long | ouverture de l'accueil | le nom et le chemin d'une bibliothèque « Not found » occupent toute la largeur de la colonne, le chemin passe à la ligne, « Locate… » et « Remove from List » sont dessous | `us078-chemin-long.png` |
| Liste plus haute que la fenêtre | 10 bibliothèques, ouverture puis molette vers le bas | le titre « Welcome to Cerata » est en haut, la liste défile jusqu'aux dernières lignes | `us078-liste-longue.png`, `us078-liste-longue-bas.png` |

**Défauts trouvés et corrigés pendant la revue.**

- Avant correctif, le même registre donnait une colonne de texte d'environ 100 px à côté des boutons d'une ligne « Not found » (nom coupé en cinq lignes, chemin en quatorze), et le titre « Welcome to Cerata » se trouvait au-dessus de l'origine du défilement : la molette vers le haut ne le montrait pas. Correctif dans `pages.rs` : la colonne est centrée dans une boîte au moins aussi haute que la fenêtre (`min_h_full`) au lieu de `my_auto`, et les actions d'une ligne « Not found » passent sous le texte. Régression : `missing_library_actions_sit_under_the_text_of_a_long_list` (`crates/cerata/src/shell/tests.rs`), en échec sans le correctif pour la position des boutons. Le titre hors écran ne se reproduit pas dans la mise en page headless de GPUI : il n'est prouvé que par les captures ci-dessus.
- `JsonStore` : un fichier corrompu qui ne pouvait pas être mis de côté (renommage refusé) était lu comme vide, et `update` l'écrasait alors sans copie. L'échec de la quarantaine est maintenant une erreur, et rien n'est écrit. Régression : `a_corrupt_file_that_cannot_be_set_aside_is_never_overwritten` (`crates/cerata_core/src/json_store.rs`), en échec sans le correctif.

Les réserves du verdict `/better-interface` ci-dessus (recapture du focus de « Create », chemin long qui passe à la ligne) sont levées par ces captures. Toujours non vérifiés : thème clair, lecteur d'écran, zoom à 200 %.
