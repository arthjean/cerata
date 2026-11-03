# Captures d'EP-015 (Windows)

## Vérification dans l'app (2026-11-03)

`cerata.exe` (build debug de l'epic) est piloté par de vraies entrées de l'OS (`SetCursorPos`, `mouse_event`, `SendKeys`) sur une copie du dossier de fixture `crates/cerata/examples/fixtures/images/`, ouvert comme espace de travail, avec un dossier `archive/` qui contient une note. La note vérifiée est `illustrated.md`, ajoutée à la fixture pour cet epic : trois images de `illustrated-assets/`, sous trois formes (destination simple avec largeur, destination entre chevrons, embed avec largeur). Fenêtre de 1216 × 808 px, thème sombre. Le disque est relu après chaque geste.

| Geste | Entrée réelle | Résultat | Capture |
|---|---|---|---|
| Ouverture | clic sur « Illustrated note » | les trois images s'affichent | `us072-avant.png` |
| Renommer la note ouverte et modifiée | clic en fin de paragraphe, frappe de « Edited », puis « offline. » 250 ms plus tard ; clic droit sur la ligne, « Rename… », Ctrl+A, « Field guide », Entrée, envoyés 853 ms après la dernière frappe | `Field guide.md` et `Field guide-assets/` (3 fichiers) ; plus de `illustrated-assets/` ; les trois destinations deviennent `<Field guide-assets/…>` et `![[Field guide-assets/diagram.svg\|200]]`, le texte tapé est gardé ; images affichées, onglet « Field guide », aucune bannière de conflit | `us072-renommage.png` |
| Déplacer par glisser-déposer | ligne de la note glissée sur le dossier `archive` | `archive/Field guide.md` et `archive/Field guide-assets/` ; texte inchangé ; images affichées | `us072-deplacement.png` |
| Dupliquer | clic droit, « Duplicate » | `archive/Field guide copy.md` et `archive/Field guide copy-assets/` (3 fichiers), destinations de la copie réécrites vers `Field guide copy-assets/`, l'original garde les siennes ; la copie s'ouvre dans un nouvel onglet, images affichées | `us073-duplication.png` |

Aucun dossier orphelin à côté de la note après les trois opérations.

**Renommage d'une note encore non enregistrée (revue d'epic, 2026-11-03).** Le premier renommage ci-dessus partait après la sauvegarde automatique. Refait sur le même build avec des frappes `keybd_event` : « x » tapé, « y » 300 ms plus tard (la seconde écriture est retenue par le délai d'une seconde entre deux sauvegardes), puis clic droit, « Rename… », Ctrl+A, « field notes », Entrée 514 ms après « y ». 30 ms avant Entrée, le titre de la fenêtre est `Field guide.md (unsaved) - Cerata` et le disque ne contient pas « x y ». Après Entrée : `field notes.md` et `field notes-assets/`, plus de `Field guide-assets/`, titre `field notes.md (unsaved)` puis `field notes.md` une fois la sauvegarde automatique passée ; le disque contient « x y » et les trois destinations `field notes-assets/…` ; images affichées, aucune bannière de conflit (`us072-renommage-non-enregistre.png`). La duplication refaite ensuite par le menu donne `field notes copy.md` et `field notes copy-assets/` (3 fichiers), destinations réécrites.

**Limite de la preuve.** La collision de noms de dossier et le retour arrière ne sont vérifiés que par les tests (`crates/cerata_core/src/fs/assets.rs`, `crates/cerata/src/sidebar/tests.rs`, `crates/cerata/src/shell/tests.rs`).

**Défaut trouvé et corrigé pendant l'implémentation.** Un résultat de sauvegarde automatique arrivé juste avant un renommage restait indexé sous l'ancien chemin et se perdait : le document gardait un texte disque périmé, et l'événement du watcher pour le nouveau chemin le marquait en conflit. `WindowView::path_renamed` déplace désormais les résultats en attente vers le nouveau chemin (`SaveResult::moved`). Régression : `renaming_an_open_edited_note_rewrites_its_buffer_and_takes_its_assets` (conflit `Modified` sans le correctif).

## Revue d'interface

La surface ajoutée se limite à des notices : `Images stay in "foo-assets": "bar-assets" already exists`, `Duplicating "foo"…`, `Failed to duplicate: {error}` (textes du PRD) et `Image links not updated: {error}` (échec d'écriture d'une note fermée, texte ajouté, hors PRD). Le skill `/better-interface` n'a pas été passé sur ces notices.
