# Stockage des notes : le fichier est l'état, la mémoire est à côté

> Statut : recherche vérifiée, **rien d'implémenté**. Rédigée le 2026-11-03.
> Base : `main` au commit `66416bc`. Versions de crates relevées sur l'API crates.io le 2026-11-03.
> Suite attendue : une ADR 0009 (bibliothèques, store local, règle d'écriture dans un dossier de notes, qui amende FR-01 du PRD), puis des epics dans le PRD.

## 0. Verdict

- **Le problème de départ n'est pas le format.** Cerata n'a ni domicile pour les notes, ni identité de note, ni mémoire. L'état d'app est indexé par chemin absolu, l'index est jeté à chaque fermeture, aucune version passée n'est conservée.
- **Cinq familles comparées** : CRDT local-first, base embarquée comme source de vérité, stockage adressé par contenu avec historique, hybrides « fichiers en projection », moteurs de sync. Aucune ne gagne seule. Les deux extrêmes échouent de façon documentée :
  - la base comme vérité : chez Logseq, le round-trip Markdown est impossible par construction (#1442), et il y a des pertes de données en sync (#781) ;
  - le CRDT comme vérité : texte dupliqué à la genèse, diffs qui fusionnent mal, historique gonflé, bibliothèques peu mûres en Rust.
- **Recommandation : le `.md` est l'état matérialisé, et une mémoire locale par bibliothèque se tient à côté.** C'est un store SQLite hors du dossier synchronisé, adressé par contenu (BLAKE3). Il tient à la fois l'index dérivé, l'historique de chaque note et la base de fusion de chaque fichier. Plus tard, les ChangeSets de l'éditeur deviennent un graphe d'événements à la Eg-walker, sans CRDT dans le buffer.
- **Ce qui est neuf** (non trouvé ailleurs dans nos recherches) : l'assemblage de quatre éléments dans un éditeur natif :
  - des fichiers canoniques ;
  - un historique adressé par contenu, indépendant du chemin ;
  - le journal d'édition de l'éditeur comme futur moteur de fusion ;
  - des segments immuables à écrivain unique, que n'importe quel tuyau transporte (dossier synchronisé, S3, iroh).

  Chaque brique existe séparément (Fossil, jj, Eg-walker, Relay, outl). Personne ne les assemble.
- **Ce n'est pas un argument Obsidian.** Le fichier reste l'état pour trois raisons d'ingénierie :
  - la panne d'un index dérivé se répare, celle d'une base source non ;
  - la réplication de base est la pire forme quand deux éditions hors ligne touchent la même note ;
  - le fichier est l'interface naturelle des scripts et des agents (Relay a réécrit son moteur en 2026-09 pour que « external tools and agents can edit your vault safely »).

## 1. Méthode et vérification

Deux passes, avec revérification de chaque affirmation structurante dans le code ou à la source.

- **Première passe** : cinq sous-agents sur le code (persistance, couche fichiers et index, parcours UI, décisions produit, état de l'art des emplacements).
- **Seconde passe** : quatre sous-agents de recherche sur sources primaires (CRDT, bases embarquées, adressage par contenu et historique, moteurs de sync).

| Périmètre | Vérifié par moi | Corrections |
|---|---|---|
| Code Cerata | `json_store.rs:97-103`, `recents.rs:15`, `fs/mod.rs:295-334`, `fs/assets.rs:12-15`, `routing.rs:110-127`, `shell/pages.rs:73-84`, `shell/mod.rs:1904-1908`, `index.rs:27-34` et `:444-478`, `editor/images.rs:259`, `docs/benchmarks.md:53-54`, `:65-67` et `:86` | aucune |
| Environnement de la machine de dev | `Documents` = `C:\Users\Arthur\OneDrive\Documents` (Known Folder Move). `dirs` 6.0.0 : `data_dir()` = Roaming, `data_local_dir()` et `cache_dir()` = Local (`dirs-6.0.0/src/win.rs:7-11`). `notify` 7.0.0 n'utilise pas `bytes_written` dans le gestionnaire ReadDirectoryChangesW (`notify-7.0.0/src/windows.rs:313`) | ajout : un débordement du tampon de surveillance passe inaperçu, et Cerata n'a pas de rescan |
| CRDT | `egwalker-paper/results/timings.json` (toutes les moyennes), résumé et corps de l'article Eg-walker, billet Automerge 3, `modeling-data.md` d'Automerge | précision : l'écart de chargement face à Automerge 0.5.9 est réel, mais yrs recharge en 8 à 13 ms dans le même fichier. L'avantage d'Eg-walker est d'environ 100x face à yrs, pas 10^5x |
| Bases embarquées | `sqlite.org/fasterthanfs.html`, `sqlite.org/wal.html` (bug WAL-reset), issues Logseq `db-test` #1442 et #781 | le benchmark local de l'agent (Python, un thread, lecture de la première ligne) valide l'intérêt d'un cache de titres, pas SQLite contre un autre format |
| Historique | `fossil-scm.org/.../tech_overview.wiki`, aide Obsidian *File recovery* (sources Markdown), `relay.md/releases`, `zed.dev/blog/introducing-deltadb` et `/introducing-delta` | aucune ; le chiffre Fossil vient de `tech_overview.wiki` |
| Sync | billet iroh v1, versions crates.io (iroh 1.0.0 le 2026-06-15, aujourd'hui 1.4.1 ; diamond-types bloqué à 1.0.0 de 2022) | l'agent annonçait « 1 à 2 ordres de grandeur » de mémoire en moins. Le résumé de l'article dit un ordre, le corps dit « 1–2 orders of magnitude less memory than the best CRDTs » « most of the time » : les deux sont exacts |

Non vérifié directement :
- les projets « Markdown + Yjs fantôme » (Baalda, opbox, outl, Muesli), lus par l'agent mais non recontrôlés ;
- les statuts de Turso, sled et libSQL, et les chiffres de démarrage de Logseq (PR #11774), relevés par l'agent ;
- le détail des modèles E2EE ;
- l'exécution en arrière-plan d'iroh sur iOS ;
- la version de SQLite embarquée par rusqlite 0.41 ;
- l'impact des nouvelles dépendances sur la taille du binaire.

## 2. État actuel de Cerata

| Brique | Où | Constat |
|---|---|---|
| Emplacement des notes | `shell/pages.rs:73-84`, `shell/commands.rs:366-381` | Aucun dossier par défaut, et « Open File » fait du dossier parent un workspace. C'est ainsi que `C:\dev\cerata\docs` est devenu un workspace, restauré ensuite à chaque lancement |
| État d'app | `%APPDATA%\cerata` (Roaming) : `sessions.json`, `pins.json`, `recent_*.json` | Les clés sont des chemins absolus. Un dossier déplacé rend l'état orphelin, et les chemins diffèrent d'une machine à l'autre |
| Durabilité des stores JSON | `json_store.rs:97-103` | Une erreur de lecture transitoire renvoie `T::default()`, que `update` réécrit ensuite par-dessus le fichier |
| Index | `index.rs:27-34` | En mémoire seulement (chemin, nom, mtime), reconstruit à chaque ouverture : 457 ms pour 50k fichiers, cache chaud |
| Titres | `fs/titles.rs`, `docs/benchmarks.md:86` | Relus à chaque ouverture : 2 à 30 s pour 50k notes |
| Embeds par nom | `editor/images.rs:259`, `index.rs:444-478` | Un parcours complet du dossier à chaque résolution |
| Surveillance | `watcher.rs`, `notify` 7.0.0 | Pas de rescan ; un débordement passe inaperçu |
| Historique | aucun | Aucune version passée. Un conflit d'autosave est détecté par contenu (`save.rs:286-295`), jamais fusionné |
| Identité de note | aucune | La note est son chemin ; `{stem}-assets/` ne suit que les renames faits par Cerata |
| Buffer | ADR 0003 | `String` + index de lignes, ChangeSets à la CodeMirror 6 : ce sont déjà des opérations par index |

## 3. Les cinq familles comparées

| Famille | Exemples | Garanties | Mesures clés | Maturité Rust (2026-11-03) | Sortie des données | Pour Cerata |
|---|---|---|---|---|---|---|
| 1. CRDT comme vérité, `.md` en projection | pushwork, darn (Ink & Switch), md-crdt, Baalda, Muesli | Convergence automatique au caractère | Automerge 0.5.9 sur les traces Eg-walker : rechargement 485 à 24 636 ms, 241 à 848 Mo au repos. Automerge 3 : Moby Dick tient en 1,3 Mo, contre 700 Mo en v2 | automerge 0.12.0, yrs 0.28.0, loro 1.17.1 | `.md` en projection, historique dans un format propre | Rejeté comme vérité : genèse dupliquée, diff qui n'est pas l'intention, historique gonflé |
| 2. Base embarquée comme vérité | Logseq DB, Bear, Anytype, Capacities, Craft, Notion | Requêtes, identité de bloc | Logseq : démarrage de 20 s à 1-2 s, grâce au chargement paresseux (PR #11774) | rusqlite 0.41.1 ; turso 0.8.3 (pré-1.0) ; redb 4.4.0 ; fjall 3.1.13 | Export avec pertes (Logseq #1442) | Rejeté comme vérité |
| 3. Base comme index dérivé | SiYuan, Obsidian (IndexedDB) | Reconstructible depuis les fichiers | Mesure locale : 50k titres en 96 ms depuis SQLite, contre 2,9 à 3,8 s depuis les fichiers à chaud et 196 s juste après leur création (antivirus probable). FTS5 ≈ 0,93x le texte | rusqlite mûr ; tantivy 0.27.0 | Totale (ce sont les fichiers) | Retenu |
| 4. Adressage par contenu, historique | git, Fossil, jj, Pijul, iroh-blobs, restic | Intégrité, time travel | Fossil : 7,1 GB d'artefacts en moins de 97 MB (74:1). SQLite lit et écrit des blobs de 10 Ko 35 % plus vite que des fichiers séparés | gix 0.90.0 (n'écrit pas de packs delta) ; jj-lib 0.46.0 (0.x, une version par mois) ; Pijul beta GPL-2.0 ; blake3 mûr | `git fast-export` à la Fossil | Retenu comme modèle (Fossil), pas comme dépendance |
| 5. Moteurs de sync | Linear, Zero, Electric, PowerSync, Jazz, Ditto, Evolu ; transports iroh, Syncthing | Métadonnées en LWW | Zero refuse l'écriture hors ligne. Evolu fait du « last writer wins per column » | iroh 1.4.1, wire stable depuis 1.0 (2026-06-15), bindings Swift | Variable | Moteurs rejetés (serveur requis, LWW sur le corps des notes) ; iroh retenu comme tuyau futur |

**Eg-walker, l'algorithme qui rend la famille 4 fusionnable** (Gentle et Kleppmann, EuroSys 2025). L'état persistant est le texte brut, et l'historique est un graphe d'événements à part, chargé seulement pour fusionner. Toutes les mesures sont tirées de `results/timings.json` du dépôt de l'article :

| Mesure | Résultat |
|---|---|
| Chargement d'un document | 0,012 à 0,121 ms |
| Fusion complète d'une trace | 1,77 à 82,6 ms |
| Fusion de la trace A2 | 23,5 ms avec Eg-walker, 3 664 266 ms (environ une heure) en OT |
| Surcoût du graphe d'événements sur disque | entre 20 % et 3 fois la taille du texte |
| Mémoire en édition | le texte seul, « no CRDT metadata » |

Le problème est la maturité. Diamond Types 2.0, l'implémentation de référence, n'est pas publiée sur crates.io. Loro 1.17.1 s'en inspire, mais garde un identifiant par caractère et exige une histoire commune à toutes les répliques.

## 4. Pourquoi le fichier reste l'état

1. **Asymétrie des pannes.** Un index dérivé désynchronisé se reconstruit (Obsidian : « it is possible for the data to get out of sync », d'où un bouton de reconstruction). Une base source corrompue ou un export infidèle est irréversible : chez Logseq, « no text export or import can round-trip » et « this cannot be fixed one bug at a time » (#1442).
2. **Les conflits.** Trois formes de stockage, jugées quand deux machines éditent la même note hors ligne :
   - la réplication de base perd une des deux éditions sans signal (LWW chez Linear, Evolu, Ditto, Turso Sync ; Logseq 2.0.1, #781 : « It overwrites the whole line ») ;
   - la sync de fichiers échoue bruyamment mais sans perte (copie de conflit) ;
   - un journal d'opérations fusionne, à condition d'avoir gardé la version de base.

   Fichiers plus versions de base, c'est la seule combinaison qui donne à la fois l'interopérabilité et une fusion sans perte.
3. **La performance ne départage pas.** L'avantage d'une base source est le démarrage à chaud, et un store dérivé le donne aussi : 96 ms contre 3 s pour 50k titres en mesure locale. Une sauvegarde durable coûte 1,06 ms au p50 en fichier atomique, contre 0,45 ms pour SQLite en `synchronous=FULL` : négligeable dans les deux cas.
4. **L'ouverture vers les agents et les outils.** Relay (2026-09-09) : « changes made directly to files on disk are picked up and merged, so external tools and agents can edit your vault safely ». Zed construit DeltaDB pour le code avec la même idée : « you can mount the whole worktree to disk whenever you want ».

Le coût honnête : l'identité de note et l'historique ne viennent pas gratuitement du système de fichiers. Le store doit les reconstruire, et détecter les renames faits hors de Cerata.

## 5. Architecture recommandée

### 5.1 Bibliothèques

**Registre.**
- Fichier `data_dir/libraries.json` contenant `{id, path, name, last_opened}`, avec un id opaque. Il remplace `recent_workspaces.json` : les entrées qui existent encore sont reprises.
- Prérequis : corriger `json_store` pour qu'une erreur de lecture échoue au lieu d'écrire la valeur vide.

**État d'app.**
- Sessions et épingles sont indexées par id de bibliothèque, avec des chemins relatifs à la racine (séparateur `/`).
- Une bibliothèque déplacée hors de Cerata devient « introuvable », avec une action « Localiser… ». Le store la reconnaît aussi par contenu : les hashes de ses fichiers correspondent aux têtes connues.

**Accueil et création de notes.**
- L'accueil propose « Créer ma bibliothèque » (chemin proposé, modifiable), « Ouvrir un dossier existant », et la liste des bibliothèques.
- Ctrl+N sans bibliothèque ouverte crée la note dans la bibliothèque par défaut.
- « Open File » ouvre une fenêtre compacte, jamais un workspace.
- Un sélecteur de bibliothèque est accessible depuis la palette.

**Avertissement cloud.** Il s'affiche si le chemin est sous `%OneDrive%` ou `~/Library/Mobile Documents`, ou si les fichiers portent l'attribut `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`.

### 5.2 Le store

**Emplacement : `dirs::data_local_dir()/cerata/libraries/<id>/store.sqlite`.**
- Sur un disque Local, jamais sur Roaming : un store de plusieurs centaines de Mo n'a rien à faire dans un profil itinérant.
- Pas dans `cache_dir()` : l'historique est une donnée, pas un cache, et `~/Library/Caches` peut être purgé sous macOS.
- Jamais dans le dossier de notes : WAL « does not work over a network filesystem » (`wal.html`), et copier une base avec sa journalisation en cours la corrompt (`howtocorrupt.html`).

Esquisse du schéma :

```sql
-- Le dossier tel que vu : projection reconstructible.
CREATE TABLE note (
  id       INTEGER PRIMARY KEY,  -- identité locale, survit aux renames
  path     TEXT NOT NULL UNIQUE, -- relatif à la racine, séparateur '/'
  head     BLOB,                 -- BLAKE3 du dernier contenu connu : base de fusion
  size     INTEGER,              -- (size, mtime_ns) : clé de réconciliation rapide
  mtime_ns INTEGER,
  title    TEXT,
  deleted  INTEGER               -- tombstone datée, pour détecter les renames
);
-- Contenus immuables adressés par hash.
CREATE TABLE blob (
  hash  BLOB PRIMARY KEY,        -- BLAKE3, 32 octets
  codec INTEGER NOT NULL,        -- brut, zstd ou delta
  base  BLOB,                    -- blob de référence d'un delta
  bytes BLOB NOT NULL
);
-- Chaîne de versions par note.
CREATE TABLE version (
  note   INTEGER NOT NULL REFERENCES note(id),
  seq    INTEGER NOT NULL,
  hash   BLOB NOT NULL REFERENCES blob(hash),
  parent BLOB,                   -- hash de la version précédente
  cause  TEXT NOT NULL,          -- save, external, restore, rename, merge, conflict
  device TEXT NOT NULL,
  ts     INTEGER NOT NULL,
  PRIMARY KEY (note, seq)
);
```

Liens, titres de sections et FTS5 viennent quand une fonctionnalité les demande. FTS5 peut s'appuyer sur la table `blob` comme contenu externe, si bien que la même copie sert à la recherche et à la fusion.

Il faut un SQLite embarqué de version 3.51.3 ou plus : le bug WAL-reset est « likely present in all version of SQLite from 3.7.0 [...] through 3.51.2 » et corrigé en 3.51.3 (`wal.html`).

### 5.3 Écriture et lecture

**Sauvegarde.**
1. Le `.md` est écrit d'abord, de façon atomique comme aujourd'hui : il est durable avant tout le reste.
2. Ensuite viennent BLAKE3, zstd et l'insertion de la version, regroupés en une transaction par fenêtre d'environ 500 ms. Si l'app plante entre les deux, la réconciliation suivante rattrape l'écart sous la cause `external`.
3. Coût par sauvegarde : un hash et une compression sur quelques Ko, plus une insertion à 0,04 ms au p50 en `synchronous=NORMAL` (mesure locale). C'est négligeable devant la cadence d'autosave d'une seconde.

**Ses propres écritures.** Aujourd'hui, Cerata les reconnaît par longueur plus un hash 64 bits gardé 10 s (`self_writes.rs`). Demain, il compare le BLAKE3 du disque à `note.head` : c'est persistant et sans fenêtre de temps.

**Contrôle avant autosave.** Aujourd'hui, Cerata relit le fichier entier (`save.rs:286-295`). Demain, il compare d'abord `(size, mtime_ns)` et ne relit que si ces valeurs ont changé. Les mtimes trop proches de la dernière écriture restent suspects, comme le « racy git ».

**Démarrage.**
1. Ouvrir le store : 0,22 ms en mesure locale.
2. Afficher l'arbre avec les titres tirés de `note` : 96 ms pour 50k en mesure locale.
3. Lancer la réconciliation en arrière-plan.

### 5.4 Réconciliation

Une seule routine et quatre déclencheurs :
- le démarrage ;
- le retour du focus ;
- une perte suspecte de la surveillance (débordement de `notify`, `IN_Q_OVERFLOW` sous Linux) ;
- un minuteur.

C'est le rôle que le protocole de jeton de FSMonitor joue pour git.

Étapes :
1. Parcourir le dossier (le parcours prend déjà 457 ms pour 50k fichiers), puis comparer `(size, mtime_ns)` avec la table `note`.
2. Ne hasher que les fichiers qui ont changé.
3. Classer chaque différence :
   - **contenu modifié** : nouvelle version de cause `external`, dont le parent est la tête connue ;
   - **même hash disparu puis apparu** : rename sûr ;
   - **similarité d'au moins 50 %** dans la même fenêtre : rename probable, comme git ;
   - **sinon** : suppression et création, liées par une version `rename?` réversible.

   `{stem}-assets/` suit la note par la même règle. Une sauvegarde atomique crée un nouvel inode, donc l'identité ne peut venir que du contenu.
4. Ne pas lire les placeholders cloud (`FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`) : garder leur titre connu et les marquer hors ligne. Ainsi, une réconciliation ne télécharge jamais tout le dossier.
5. Traiter les copies de conflit des services de sync comme une branche de la note X : deux versions de même parent, et une proposition de fusion. Sont concernés `X (conflicted copy …).md` (Dropbox), `X.sync-conflict-*.md` (Syncthing) et `NSFileVersion` (iCloud). Le fichier parasite disparaît une fois la fusion faite.

### 5.5 Historique et fusion

- **Une chaîne de versions par note**, pas un instantané du dossier. À la manière de git, un dossier plat de 50k notes réécrit un objet tree d'environ 2,9 Mo à chaque sauvegarde (calcul de l'agent, inférence).
- **Rétention à la Time Machine** :
  - tout est gardé pendant 24 h ;
  - puis une version par heure jusqu'à 30 jours ;
  - puis une par jour ;
  - avec des épinglages manuels, une GC des blobs orphelins, et une vraie purge qui réécrit les blobs (la leçon d'Upwelling sur le droit à l'oubli).
- **Fusion.** Quand le buffer a des modifications non sauvegardées et que le disque a changé, Cerata fait une fusion à trois voies :

  | Rôle | Source |
  |---|---|
  | Base | `head` |
  | Nôtre | le buffer |
  | Leur | le disque |

  Le diff se fait comme dans Zed : lignes (histogramme) puis mots, après normalisation des fins de ligne et du BOM. Un recouvrement s'affiche comme deux branches à relire. Aucun côté n'est jamais perdu, puisque les deux sont dans le store. Aujourd'hui, Cerata ne fait que détecter le conflit.
- **Compression delta** : 74:1 chez Fossil, mais sur du code. Le ratio sur des notes Markdown n'a pas été trouvé : il faut le mesurer.

### 5.6 Graphe d'événements (après v1)

- **Journal des ChangeSets.** Chaque ChangeSet sauvegardé est journalisé avec `(device, seq)` et des parents, c'est-à-dire le hash du snapshot de base. C'est exactement le graphe d'événements d'Eg-walker (« Records operations with simple indices »).
- **Ce que ça apporte** :
  - un time travel à la frappe ;
  - une provenance par caractère : toi, une autre machine, un agent passé par Cerata, ou un éditeur externe. Pour ce dernier, la provenance reste générique, car l'OS ne dit pas qui a écrit un fichier.
- **Genèse déterministe.** L'événement racine d'une note dérive du hash de son premier contenu connu. Deux machines qui ingèrent le même `.md` obtiennent alors la même racine, ce qui évite la duplication que Yjs, Automerge et Loro documentent.
- **Implémentation.** Aucune crate Rust maintenue ne fournit Eg-walker. Trois options :
  - l'écrire d'après l'article et l'implémentation TypeScript de référence ;
  - vendoriser Diamond Types (ISC, un seul mainteneur) ;
  - utiliser Loro (mûr, mais un id par caractère et une histoire commune obligatoire).

  À trancher avec l'epic de sync.

### 5.7 Sync (hors v1, mais le format la permet)

- **Segments immuables.** Ils sont nommés par hash, chaque appareil n'écrit que les siens, et ils sont chiffrés côté client (modèle Argon2id + XChaCha20-Poly1305 de Standard Notes et Notesnook). Comme aucun fichier n'a deux écrivains, n'importe quel tuyau les transporte sans conflit :
  - le dossier Syncthing, iCloud ou OneDrive que l'utilisateur a déjà ;
  - un bucket S3 ou un serveur WebDAV ;
  - iroh quand deux appareils sont allumés : environ 9 réseaux sur 10 permettent une connexion directe, et les relais ne lisent pas le trafic et peuvent être auto-hébergés.

  Aucun serveur Cerata.
- **Métadonnées partageables** (épingles, tags) : LWW par champ. Les sessions restent propres à chaque appareil.
- **GC à quiescence.** L'ensemble des répliques est fini et connu (trois machines). Dès qu'elles ont toutes acquitté une version, l'historique antérieur peut être compacté. Le problème « on ne sait pas quand un pair reviendra » disparaît pour ce cas d'usage.

## 6. Ce qui est neuf, et ce qui ne l'est pas

Déjà banal en 2026 :
- `.md` canonique avec un Yjs fantôme et un diff des éditions externes : au moins six projets (Baalda, opbox, outl, Muesli, lumi, sundial) ;
- un historique par chemin (VS Code Local History ; Obsidian File Recovery, 7 jours par défaut, stocké « with the absolute path to the note ») ;
- la base comme vérité (Logseq).

Non trouvé dans nos recherches (non exhaustives) : l'assemblage dans un éditeur natif de ces cinq éléments :
- des fichiers canoniques ;
- un historique adressé par contenu qui survit aux renames et aux déplacements ;
- le journal d'édition de l'éditeur lui-même comme graphe de fusion, sans CRDT en mémoire ;
- une genèse déterministe ;
- des segments à écrivain unique transportables par n'importe quel outil de sync.

L'équivalent le plus proche est DeltaDB de Zed (« Where Git captures a snapshot at each commit, DeltaDB captures every operation in between »), en bêta privée depuis le 2026-08-12. Il vise le code, pas les notes, et sa licence n'est pas publiée.

## 7. Rejeté

- **Base comme source de vérité** : interopérabilité, git et export fidèle perdus. La conversion n'a pas réglé la perte en sync (Logseq #781). Logseq a dû scinder son produit, car « Every feature, bug fix, and UX improvement must be built twice ».
- **CRDT vivant par note dans le buffer** (Yjs, Automerge) : mémoire, genèse, intention perdue par les diffs. C'est aussi contraire à l'ADR 0003, qui reste valable.
- **git, jj ou Pijul embarqués comme moteur d'historique** :
  - gix n'écrit pas de packs compressés en delta ;
  - jj-lib est en 0.x avec une version par mois ;
  - Pijul est en beta sous GPL-2.0 ;
  - un snapshot du dossier entier coûte cher à chaque sauvegarde ;
  - un historique placé dans le dossier passe par les clients de sync (« conflicted copy » chez Dropbox ; jj : « repository corruption is possible »).
- **Store dans le dossier de notes** : WAL hors réseau, `howtocorrupt`, clients de sync.
- **Moteurs de sync produits** : serveur applicatif requis, LWW sur le corps des notes.
- **Autres bases** :
  - cr-sqlite : son mainteneur est absent pour « at least 1-2 years » ;
  - sled : toujours « beta » ;
  - libSQL : remplacé par la réécriture Rust de Turso ;
  - Turso : pré-1.0, et son mode d'écriture concurrente ne gagne qu'avec plusieurs écrivains.
- **Notes dans le dossier de données de l'app** : caché, sur Roaming sous Windows, et Obsidian l'interdit explicitement (« Don't create a vault in the system folder »).

## 8. Risques et points ouverts

- **Taille de l'historique** : le ratio de compression sur des notes n'est pas mesuré. Il faut rejouer un an de sauvegardes synthétiques sur une copie d'un vrai dossier, et comparer zstd, zstd avec dictionnaire et delta.
- **UX d'un historique à la frappe** : Patchwork montre qu'il faut des regroupements dynamiques.
- **Faux renames par similarité** : le seuil est à régler sur un corpus réel.
- **mtime « racy »** : sur NTFS, APFS et ext4, la précision suffit, mais une écriture dans le même tick que la nôtre reste à traiter comme suspecte.
- **Placeholders cloud** : leurs titres peuvent être périmés tant que le fichier n'est pas téléchargé.
- **Identité entre machines avant la sync** : ids locaux en v1 ; la genèse déterministe reste à valider sur trois machines.
- **Eg-walker en Rust** : à écrire ou à vendoriser.
- **Nouvelles dépendances dans `cerata_core`** : rusqlite (feature `bundled`, du C compilé sur les trois OS), blake3, zstd. Leur impact sur le binaire n'est pas mesuré.
- **PRD à amender** : la contrainte « la seule écriture de Cerata dans un workspace est `{root}/.cerata/config` » (`prd-cerata-port.md:114`, FR-01 `:1547`) est déjà contredite par `{stem}-assets/`. L'ADR 0009 doit dire ce que Cerata écrit dans une bibliothèque : les notes, leurs assets, `.cerata/config`, et un éventuel `.cerata/id`. La promesse « sans index propriétaire » (`:58`) reste tenue au sens où supprimer le store ne perd que l'historique et les bases de fusion : tout le reste se reconstruit depuis les fichiers.

## 9. Décisions à prendre

1. **Emplacement par défaut** :
   - `Documents\Cerata` (chez Arthur, dans OneDrive) : synchro gratuite, mais placeholders et copies de conflit, dont le store absorbera ces dernières ;
   - ou `~/Cerata` : local, sans surprise, sans sauvegarde.
2. **Marqueur `.cerata/id`** dans la bibliothèque, écrit une seule fois pour reconnaître un dossier déplacé ou synchronisé, ou aucun marqueur.
3. **Rétention par défaut** de l'historique, et forme de la purge.
4. **Assets dans le store** : octets compris (historique complet des images) ou hash seulement (détection des renames).
5. **Plus tard** : Eg-walker maison, Diamond Types vendorisé, ou Loro.

## 10. Découpage proposé

| Epic | Contenu | Signal de vérification |
|---|---|---|
| A. Fondations (immédiat) | Correctif `json_store` (échec au lieu d'écrasement), ADR 0009, registre de bibliothèques et migration, état d'app indexé par id avec chemins relatifs, accueil, Ctrl+N, « Open File » en fenêtre compacte, sélecteur, avertissement cloud | Tests : lecture en erreur sans écrasement, migration des récents, bibliothèque déplacée retrouvée |
| B. Store et index persistant | `store.sqlite`, table `note` avec titres, réconciliation à quatre déclencheurs, placeholders, table des noms d'assets (supprime le parcours par embed) | Nouvelle ligne dans `docs/benchmarks.md` : « 50k notes, du lancement aux titres affichés, store chaud ». Tests : modification hors surveillance rattrapée, rename détecté par hash |
| C. Mémoire | Versions par note, rétention, panneau d'historique, éditions externes en versions, copies de conflit en branches, fusion à trois voies | Test de fusion sans perte. Mesure du ratio de compression sur un corpus de notes |
| D. Après v1 | Graphe d'événements Eg-walker, provenance, sync par segments (dossier, S3, iroh) | Fusion de deux branches hors ligne sur trois machines, sans duplication de genèse |

## Sources

CRDT et local-first :
- Gentle et Kleppmann, *Collaborative Text Editing with Eg-walker: Better, Faster, Smaller*, EuroSys 2025 : <https://arxiv.org/abs/2409.14252>, version HTML <https://arxiv.org/html/2409.14252>. Mesures : <https://github.com/josephg/egwalker-paper/blob/master/results/timings.json>.
- Automerge 3 (mémoire, Moby Dick) : <https://automerge.org/blog/automerge-3/>. Granularité des documents : <https://github.com/automerge/automerge.github.io/blob/main/content/docs/cookbook/modeling-data.md>.
- Loro, Event Graph Walker : <https://loro.dev/docs/concepts/event_graph_walker>.
- Yjs, duplication sans histoire commune : <https://discuss.yjs.dev/t/whats-the-correct-way-to-set-default-content-for-y-prosemirror/1129>.
- Ink & Switch : Patchwork <https://www.inkandswitch.com/project/patchwork/>, darn <https://github.com/inkandswitch/darn>, pushwork <https://github.com/inkandswitch/pushwork>, Keyhive <https://www.inkandswitch.com/keyhive/notebook/05/>.
- Zed : modèle CRDT du buffer <https://zed.dev/blog/crdts>, rechargement par diff (`text_diff`, `apply_diff`, `has_conflict`) <https://github.com/zed-industries/zed/blob/0294cfb58e8e24d12be7596f56bdb7a53e3dfd2f/crates/language/src/buffer.rs>, DeltaDB <https://zed.dev/blog/introducing-deltadb> et <https://zed.dev/blog/introducing-delta>.
- Weidner et Kleppmann, *The Art of the Fugue* (entrelacement) : <https://arxiv.org/abs/2305.00583>.

Bases embarquées :
- SQLite : <https://www.sqlite.org/fasterthanfs.html>, <https://www.sqlite.org/wal.html>, <https://www.sqlite.org/howtocorrupt.html>, <https://www.sqlite.org/fts5.html>, <https://www.sqlite.org/appfileformat.html>.
- Logseq DB : <https://github.com/logseq/docs/blob/master/db-version.md>, issues <https://github.com/logseq/db-test/issues/1442> et <https://github.com/logseq/db-test/issues/781>.
- cr-sqlite, maintenance : <https://github.com/vlcn-io/cr-sqlite/issues/444>.
- Bear, emplacement et avertissements : <https://bear.app/faq/where-are-bears-notes-located/>.
- Obsidian, données et cache de métadonnées : <https://obsidian.md/help/data-storage>. File Recovery : <https://github.com/obsidianmd/obsidian-help/blob/master/en/Plugins/File%20recovery.md>.

Adressage par contenu et historique :
- Fossil, aperçu technique (74:1) : <https://fossil-scm.org/home/doc/trunk/www/tech_overview.wiki>.
- jj, concurrence et opération log : <https://jj-vcs.github.io/jj/latest/technical/concurrency/>.
- git : split index <https://git-scm.com/docs/git-update-index>, FSMonitor <https://github.blog/engineering/infrastructure/improve-git-monorepo-performance-with-a-file-system-monitor/>.
- Relay, ingestion des éditions disque (0.8.12) : <https://relay.md/releases>, <https://github.com/no-instructions/Relay/issues/88>.
- Dropbox, copies en conflit : <https://help.dropbox.com/organize/conflicted-copy>. Apple `NSFileVersion` : <https://developer.apple.com/documentation/foundation/nsfileversion>.

Sync et transports :
- iroh 1.0 : <https://www.iroh.computer/blog/v1>, relais <https://docs.iroh.computer/concepts/relays>.
- Syncthing, conflits : <https://docs.syncthing.net/users/syncing.html>.
- Zero, hors ligne : <https://zero.rocicorp.dev/docs/offline>. Evolu, sync : <https://www.evolu.dev/docs/sync>.
- Jazz, leçons de la v1 : <https://jazz.tools/blog/what-we-learned-from-classic-jazz>.
- outl, sync par journal à écrivain unique : <https://github.com/outlmd/outl/blob/main/docs/sync.md>.

Plateformes :
- `dirs` et `dirs-sys` (Known Folders Windows) : <https://github.com/dirs-dev/dirs-sys-rs>.
- OneDrive Known Folder Move : <https://learn.microsoft.com/en-us/sharepoint/redirect-known-folders>. Fichiers à la demande (cfapi) : <https://learn.microsoft.com/en-us/windows/win32/cfapi/build-a-cloud-file-sync-engine>.
- ReadDirectoryChangesW (débordement) : <https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw>. inotify : <https://man7.org/linux/man-pages/man7/inotify.7.html>.
