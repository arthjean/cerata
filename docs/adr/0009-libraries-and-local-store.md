# ADR 0009 : bibliothèques, store local et règle d'écriture

> Statut : **décidé** le 2026-11-03. Epic : EP-016 (PRD stockage, `tasks/prd-cerata-stockage.md`), story US-075 ; s'applique aussi à EP-017 et EP-018. Amende FR-01 du PRD de portage (`tasks/prd-cerata-port.md:1547`) et sa contrainte « la seule écriture de Cerata dans un workspace est `{root}/.cerata/config` » (`:114`). Références : `docs/research/stockage-notes.md` §5 (architecture) et §9 (décisions à prendre).

## Contexte

Un workspace était un dossier quelconque, connu par son chemin absolu canonique. Sessions, épingles et récents étaient indexés par ce chemin (`session.rs`, `pins.rs`, `recents.rs`) : un dossier déplacé perdait tout son état, et le même dossier n'avait pas la même clé d'une machine à l'autre. L'accueil ne proposait aucun emplacement, et « Open File » faisait du dossier parent un workspace. Sur la machine de dev, `C:\dev\cerata\docs` figurait ainsi dans `recent_workspaces.json`, et des notes personnelles ont atterri dans le dépôt.

La règle d'écriture du PRD de portage (« rien d'autre que `.cerata/config` ») était déjà contredite par les dossiers `{stem}-assets/` des images (ADR 0008, EP-012 à EP-015). Les epics EP-017 et EP-018 ajoutent un store (index, historique), qui ne doit jamais vivre dans le dossier de notes.

## Faits sourcés

- `dirs` 6.0.0 : `data_local_dir()` rend `FOLDERID_LocalAppData` sous Windows (`dirs-6.0.0/src/win.rs:11`), `data_dir()` rend `FOLDERID_RoamingAppData`. Un profil itinérant copie Roaming à chaque ouverture de session.
- SQLite : le mode WAL « does not work over a network filesystem » (sqlite.org/wal.html), et copier une base pendant qu'elle journalise la corrompt (sqlite.org/howtocorrupt.html). Un store dans un dossier synchronisé est donc exclu.
- Obsidian File Recovery garde ses instantanés « with the absolute path to the note » (aide Obsidian, cité par `docs/research/stockage-notes.md` §3) : un renommage ou un déplacement casse le lien entre une note et son historique.
- Le dossier `.cerata/` est déjà masqué de l'arbre, de l'index et du surveillant par la règle des composants cachés (`ignore.rs`, test `hidden_components_below_the_root_are_excluded`).

## Décision

1. **Identité.** Une bibliothèque a un id de 128 bits tirés du système (`getrandom`), écrit en 32 caractères hexadécimaux minuscules.
   - Il est écrit une fois dans `{root}/.cerata/id`, suivi de `\n`, de façon atomique et en création seule (lien dur d'un fichier temporaire synchronisé, `fs::atomic::atomic_create`) : un marqueur existant n'est jamais écrasé.
   - Seule exception : un dossier copié. Quand le marqueur porte l'id d'une bibliothèque encore présente à son chemin enregistré, avec le même marqueur, la copie reçoit un nouvel id et son marqueur est réécrit.
   - Un marqueur dont l'id est connu à un chemin qui n'existe plus est une bibliothèque déplacée : son entrée suit le dossier.
   - Un marqueur illisible ou mal formé est ignoré (reconnaissance par le chemin), jamais réécrit. Un dossier où rien ne peut être écrit est reconnu par son chemin seulement.
2. **Registre.** `libraries.json` dans `data_dir()/cerata`, sous `JsonStore` : `{"version": 1, "libraries": [{id, path, name, created_at, last_opened_at, is_default}]}`. Il remplace `recent_workspaces.json`, dont les dossiers encore présents sont repris une fois, au premier démarrage ; ce fichier reste en place.
3. **État d'app.** `sessions.json` et `pins.json` sont indexés par id de bibliothèque, avec des chemins relatifs à la racine et `/` pour séparateur. Un chemin relatif qui sort de la racine est rejeté au chargement. `{root, epoch}` reste l'identité d'exécution (`workspace.rs`, `WorkspaceSnapshot`).
4. **Emplacement par défaut.** `~/Cerata`, nom et emplacement modifiables dans le formulaire d'accueil. Un emplacement synchronisé (OneDrive, iCloud Drive, Dropbox, Syncthing, `~/Library/CloudStorage`) déclenche un avertissement sans bloquer la création. Le choix de `Documents\Cerata`, synchronisé par OneDrive sur la machine d'Arthur, est écarté par le PRD (option 1 du §9 de la recherche) : placeholders et copies de conflit dès le premier jour.
5. **Store.** `data_local_dir()/cerata/libraries/<id>/store.sqlite` : Local sous Windows, jamais Roaming, jamais dans `cache_dir()` (l'historique est une donnée, et `~/Library/Caches` peut être purgé), jamais dans le dossier de notes ni dans un dossier détecté comme synchronisé. Supprimer le store ne perd que l'historique et les bases de fusion : tout le reste se reconstruit depuis les fichiers.
6. **Appareil.** L'id d'appareil (EP-018, champ `device` des versions) vit dans `data_local_dir()/cerata`, jamais en Roaming : un profil itinérant le copierait sur une autre machine, qui se ferait passer pour la première.
7. **Rétention de l'historique.** Paliers : toutes les versions pendant 24 h, une par heure jusqu'à 30 jours, puis une par jour sans limite. La purge est réelle : les blobs orphelins sont supprimés de la base, et pas seulement délistés.
8. **Images.** Le store suit les fichiers des dossiers `{stem}-assets/` par leur hash seulement (détection des renommages), sans copier leurs octets dans l'historique.

## Règle d'écriture

Dans une bibliothèque, Cerata n'écrit que :

- les notes (`.md`, `.markdown`) ;
- les dossiers `{stem}-assets/` des notes et leur contenu ;
- `.cerata/config` ;
- `.cerata/id`.

Rien d'autre : ni index, ni verrou, ni cache, ni fichier temporaire qui survive à une écriture (les temporaires de `atomic_write` sont cachés et supprimés en cas d'échec). Cette règle amende FR-01 (`tasks/prd-cerata-port.md:1547`) et la contrainte de `:114`. Les dossiers créés par l'utilisateur depuis la sidebar restent permis : ils ne contiennent que des notes.

Elle est vérifiée par le test `library_write_rule` (`crates/cerata_core/tests/library_write_rule.rs`). Il ouvre une bibliothèque temporaire, y crée, enregistre, renomme, duplique et supprime des notes, importe une image et écrit un réglage, puis échoue en nommant chaque fichier hors de la règle. Le test `library_write_rule_names_a_stray_file` prouve que la vérification nomme bien les chemins fautifs.

## Options écartées

- **Aucun marqueur** : une bibliothèque déplacée hors de Cerata deviendrait introuvable, et une copie partagerait l'identité de l'original, donc ses sessions et plus tard son historique.
- **Id dérivé du chemin** : ne survit ni à un déplacement ni à un changement de machine, ce qui reproduit le défaut actuel.
- **Store dans `.cerata/`** : voyagerait par un service de sync, et SQLite en WAL n'y est pas sûr (faits ci-dessus).
- **Historique dans git** : un dossier plat de 50 000 notes réécrit un arbre d'environ 2,9 Mo à chaque sauvegarde (§5.5 de la recherche, inférence).

## Conséquences

- Ouvrir un dossier pour la première fois y écrit `.cerata/id` (33 octets). Un dossier ouvert seulement pour un fichier isolé (« Open File… ») n'en reçoit pas : il s'ouvre en fenêtre compacte (US-079).
- Les clés absolues de `sessions.json` et `pins.json` sont converties à la première lecture de leur bibliothèque. Une entrée sans bibliothèque correspondante reste en place, inutilisée.
- `RecentsStore` ne garde que les fichiers récents ; la liste des workspaces récents est le registre, plafonné à l'affichage par `workspace.max-recent-workspaces`.
- Les stores JSON échouent au lieu d'écraser (US-074) : une erreur de lecture autre que l'absence rend une `StoreError`, et un fichier corrompu est mis de côté sous `{nom}.corrupt-{AAAAMMJJ-HHMMSS}` (UTC) sans remplacer une quarantaine précédente.

## Non prouvé

- **SQLite embarqué tient ses budgets** (au plus 3 Mo de binaire en plus, 150 ms pour lire 50 000 titres, compilation sur les trois runners CI) : validé ou infirmé par US-081. Un budget dépassé amende cette ADR au profit d'un instantané `serde_json` versionné pour l'index, avec l'historique reporté.
- **L'historique en paliers grossit de moins de 50 Mo par an** pour un usage soutenu : mesuré par US-088. Le ratio de compression sur des notes Markdown n'a pas été mesuré.
- **Le marqueur `.cerata/id` est accepté** par les utilisateurs d'un dossier existant (coffre Obsidian, `docs/` versionné) : hypothèse du PRD, non mesurée.
- **Le comportement sous macOS et Linux** (dossier en lecture seule, liens durs sur les volumes réseau et FAT) : couvert par un test Unix en CI, à vérifier à la main dans EP-009 et EP-010 du portage.

## Addendum : codec de l'historique (US-088, 2026-11-03)

Cet addendum consigne la mesure que la section « Non prouvé » attendait de US-088 ; il ne change aucune décision ci-dessus.

- **Mesure.** Un an d'usage soutenu simulé par `history_bench` (300 notes de 1 à 20 Ko, 20 éditées par jour, environ 103 versions créées par jour, paliers de la décision 7 appliqués chaque jour) donne un store de 16,03 à 16,59 Mio avec le codec retenu, contre 59,12 à 62,19 Mio pour zstd seul et 32,38 à 34,16 Mio pour zstd avec dictionnaire (trois graines, `docs/benchmarks.md`).
- **Codec retenu : delta puis zstd niveau 3.** Un blob est compressé par zstd niveau 3 avec, pour dictionnaire, le contenu de la version précédente de la note (sa base). Le premier blob d'une note, et celui qui suivrait 32 deltas, est compressé seul. Lire une version décode donc au plus 32 deltas.
- **Purge et ramassage.** Supprimer un blob qui sert de base à un autre réencode d'abord ce dernier sur la plus proche base qui reste, ou seul s'il n'en reste aucune : aucun blob gardé ne dépend d'un contenu purgé.
- **Borne.** Le palier quotidien reste sans limite : la borne de 365 jours, prévue si aucun codec ne tenait 50 Mo, n'est pas appliquée.

