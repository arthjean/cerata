# Captures d'EP-017 (Windows)

## Vérification dans l'app (2026-11-03)

`cerata.exe` (build release de l'epic, copié dans un dossier jetable) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`), envoyées seulement si `cerata.exe` est au premier plan. Le dossier de données de l'app (`%APPDATA%\cerata`) est mis de côté par renommage avant le run, puis remis en place et comparé à sa sauvegarde avec `diff -r` (identique). Le dossier `%LOCALAPPDATA%\cerata` créé par le run (les stores) part à la corbeille. La bibliothèque ouverte est un dossier jetable, `Field Journal`, de sept notes dont le titre diffère du nom de fichier (cinq à la racine, deux dans `Trips`). Fenêtre de 1216 × 808 px, puis 800 × 600 px, thème sombre. Le disque, `pins.json` et le store (lecture seule avec `sqlite3`) sont relus après chaque geste.

| Story | Geste | Entrée réelle | Résultat | Capture |
|---|---|---|---|---|
| US-083 | Premier lancement | lancement sur `Field Journal`, store absent | les titres s'affichent ; log `reconciled 7 notes … 7 read` ; le store est créé sous `%LOCALAPPDATA%\cerata\libraries\{id}\store.sqlite` | (pas de capture) |
| US-085 | Préparation | clic sur « Draft letter to Camille », clic droit puis « Pin », clic dans le texte, saisie de « Typed before the rename. » | la note est ouverte (onglet `letter`), épinglée, et la saisie est enregistrée sur le disque | `us085-avant.png` |
| US-085 | Renommage hors de Cerata | `Rename-Item letter.md letter-to-camille.md` dans PowerShell, app ouverte | l'onglet devient `letter-to-camille`, la note reste dans « Pinned », le texte et le curseur restent en place ; `pins.json` contient `letter-to-camille.md` ; dans le store, la ligne garde l'id 6 avec le nouveau chemin | `us085-apres.png` |
| US-083 | Relance, store chaud | Alt+F4, puis relance | l'arbre et l'épingle s'affichent avec leurs titres ; log `read 7 notes from the store in 4.4983ms`, puis `reconciled 7 notes … 0 read` : aucun fichier n'est ouvert | `us083-relance.png` |
| US-082 | Store corrompu | Alt+F4, 4 096 octets aléatoires écrits dans `store.sqlite`, relance | notice « Cerata rebuilt this library's index. Older versions were set aside in C:\Users\Arthur\AppData\Local\cerata\libraries\8ca6…\store.sqlite.corrupt-20261103-162848. » ; le store et ses `-wal`, `-shm` sont mis de côté sous ce nom ; un store neuf est rempli (`7 read`) | `us082-reconstruit.png` |
| US-082 | Même notice, fenêtre étroite | fenêtre ramenée à 800 × 600 px | le chemin passe à la ligne, le bouton de fermeture reste dans la notice | `us082-reconstruit-etroit.png` |
| US-082 | Store d'une version future | fermeture, `PRAGMA user_version = 999` sur le store, relance | notice « This library's index was made by a newer Cerata. History is unavailable until you update. » ; la bibliothèque s'ouvre avec ses titres ; le SHA-1 du store est le même avant et après (`2211a1fe…`) | `us082-version-future.png` |

**Défaut trouvé et corrigé pendant le run.** Le chemin de la notice « rebuilt » ne passait pas à la ligne. À 1216 px, la notice faisait 1140 px de large et recouvrait les boutons Recherche et Réglages de la barre latérale. À 800 px, le texte était coupé des deux côtés et le bouton de fermeture sortait de la fenêtre (`us082-notice-avant-correctif.png`). Correctif dans `crates/cerata/src/shell/pages.rs` (`notice`) : la notice garde 16 px de marge dans la fenêtre et ne dépasse pas 560 px (`NOTICE_MAX_WIDTH`). Son texte passe à la ligne, et les boutons gardent leur taille. Régression : `a_long_notice_wraps_and_keeps_its_dismiss_button_in_the_window` (`crates/cerata/src/shell/tests.rs`). Sans le correctif, ce test échoue : notice de 2896 px de large dont l'origine est à -848 px, dans une fenêtre de 1200 px. Les captures `us082-reconstruit*.png` sont prises après le correctif.

**Non vérifié dans l'app.** Le renommage par Cerata elle-même (aucun événement `Renamed`) n'est prouvé que par `renaming_an_open_edited_note_rewrites_its_buffer_and_takes_its_assets` (`crates/cerata/src/sidebar/tests.rs`). La correction d'un titre périmé à l'ouverture n'est prouvée que par les tests de `cerata_core` (`workspace.rs`, `reconcile/tests.rs`). La remise à jour des titres de la palette « Recent » (`refresh_recent_titles`) n'est ni capturée ni couverte par un test. Le chiffre de 50 000 notes est dans `docs/benchmarks.md`.

## Revue d'interface (`/better-interface`)

**Périmètre.** Les surfaces livrées par l'epic : les titres de l'arbre et de la section « Pinned » au lancement, lus depuis le store (`crates/cerata/src/sidebar/mod.rs`, `read_listing` et `with_titles`) ; les deux notices du store (`crates/cerata_core/src/store.rs`, `StoreNotice::message`, affichées par `pages::notice`) ; l'onglet et l'épingle qui suivent un renommage fait hors de Cerata (`crates/cerata/src/shell/mod.rs`, `renamed_outside`) ; les titres des récents (`crates/cerata/src/shell/commands.rs`, `refresh_recent_titles` ; `crates/cerata/src/palette.rs`, `relabel_recents`). Aucun nouveau composant ni mouvement : l'epic réutilise la notice, les lignes de la barre latérale et les onglets. Conventions lues : `AGENTS.md`, `docs/captures/ep-007/` et `ep-016/`. Les six skills de domaine sont chargés : accessibilité, mise en page, rédaction, typographie, couleurs, UI.

| Domaine | Éléments inspectés | Résultat |
|---|---|---|
| Accessibilité | `pages.rs` `notice` : `Role::Status`, `aria_label`, bouton « Dismiss » nommé ; la notice reste jusqu'à sa fermeture | 1 constat (le même que Mise en page) |
| Mise en page | captures à 1216 et 800 px, `pages.rs` `notice` | 1 HIGH corrigé, 1 LOW |
| Rédaction | les deux messages, imposés mot pour mot par le PRD (US-082) | 1 LOW |
| Typographie | retour à la ligne du chemin, taille 13 px héritée de la notice | Clair après correctif |
| Couleurs | contraste mesuré sur les captures | Clair |
| UI | aucune nouvelle surface ni animation | Clair |

**Contraste mesuré sur les captures (thème sombre).** Texte de la notice 18,41:1 (`#fcfcfc` sur `#111111`), titres de la barre latérale 15,32:1 (`#fcfcfc` sur `#232323`). Tout passe.

| Sévérité | Domaine | Emplacement | Avant | Après | Pourquoi |
|---|---|---|---|---|---|
| HIGH | Mise en page | `crates/cerata/src/shell/pages.rs:445` (`notice`) | boîte sans largeur maximale, texte d'un seul tenant : à 800 px le chemin est coupé et « Dismiss » sort de la fenêtre | corrigé : `max_w(560 px)`, marge de 16 px, texte en `flex_1().min_w_0()`, boutons en `flex_shrink_0()` | contenu tronqué sans accès au reste, et contrôle hors d'atteinte (déclencheur) |
| LOW | Mise en page | `crates/cerata/src/shell/pages.rs:445` | à 800 px, la notice sur trois lignes recouvre le bouton Réglages tant qu'elle est ouverte | consigné, non corrigé : la notice se ferme d'un clic, et sa place en bas au centre précède l'epic | contrôle masqué de façon passagère |
| LOW | Rédaction | `crates/cerata_core/src/store.rs:85` | « Older versions were set aside… » alors que l'historique n'existe pas encore | consigné : texte imposé par le PRD, juste une fois l'historique livré | terme en avance sur la fonction |

**Non vérifié.** Thème clair, lecteur d'écran (annonce de la notice), zoom à 200 %, palette « Recent » après remise à jour des titres.

**Verdict.** Approuvé : le seul HIGH est corrigé, prouvé par le test de régression et par les captures `us082-reconstruit.png` et `us082-reconstruit-etroit.png`. Les deux LOW restent à faire.
