# Benchmarks et machines de référence

Ce document fixe les machines sur lesquelles Cerata est mesuré et la procédure qui mesure Writer (writer-computer au commit `79b744c`) pour lui servir de point de comparaison. Toute valeur citée dans une ADR ou une story vient d'une de ces machines, avec la commande qui l'a produite.

## Machines de référence

| OS | Machine | Processeur | Mémoire | GPU | Stockage | Système | Statut |
|---|---|---|---|---|---|---|---|
| Windows | poste fixe d'Arthur | AMD Ryzen 7 7800X3D (8 cœurs) | 32 Go | NVIDIA RTX 4070 Ti SUPER (iGPU AMD Radeon présent) | Samsung SSD 990 PRO 1 To (NVMe) | Windows 11 Pro 10.0.26300 | désignée le 2026-10-31 |
| macOS | Mac Apple Silicon loué chez Scaleway (seule cible de Writer) | | | | | | à documenter en US-046 (EP-010) |
| Linux | machine d'Arthur (Wayland et X11 sur la même machine) | | | | | | à documenter en US-045 (EP-009) |

Règles : machine branchée sur secteur, mode performance, aucune autre application ouverte, build `--release`, trois exécutions de chauffe puis la médiane d'au moins cinq mesures. Une valeur « à froid » exige un redémarrage de la machine (ou `sudo purge` sur macOS, `echo 3 | sudo tee /proc/sys/vm/drop_caches` sous Linux) juste avant la mesure.

## Corpus

Le générateur du benchmark d'indexation produit les workspaces de mesure. Il écrit 100 notes par dossier sur deux niveaux, une pièce jointe PNG par dossier, un `.gitignore` et un dossier ignoré de 200 notes, pour exercer les mêmes règles qu'un vrai coffre.

```sh
cargo run -p cerata_core --release --example index_bench -- generate <dossier> 10000
cargo run -p cerata_core --release --example index_bench -- generate <dossier> 50000
cargo run -p cerata_core --release --example index_bench -- index <dossier> 7
```

Le même dossier généré sert à Writer et à Cerata.

## Procédure de mesure de Writer

Writer ne publie que des binaires macOS arm64 (`docs/releasing.md:89-92` de writer-computer) : ses mesures se font sur la machine de référence macOS, build de release installé depuis le DMG de la v0.8.0 ou construit au commit `79b744c` (`pnpm tauri build`).

1. **Démarrage à froid.** Redémarrer, attendre deux minutes, puis lancer `date +%s.%N; open -a Writer <workspace-10k>` et filmer l'écran à 60 i/s (QuickTime). La durée va de la commande à la première image où la sidebar affiche la liste de la racine. Cinq redémarrages, médiane.
2. **Démarrage à chaud.** Même mesure sans redémarrage, Writer quitté depuis dix secondes.
3. **Mémoire.** Workspace de 10 000 fichiers ouvert, une note ouverte, dix secondes d'inactivité, puis somme des RSS de tous les processus de l'app, processus WebKit compris : `ps -axo rss,comm | grep -Ei 'writer|webkit' | awk '{s+=$1} END {print s/1024 " Mo"}'`. Relever aussi l'empreinte mémoire du Moniteur d'activité.
4. **Ouverture d'un workspace de 10 000 fichiers.** Writer déjà lancé sur un autre dossier, ouvrir le workspace généré par Fichier > Ouvrir ; filmer à 60 i/s. La durée va du clic sur « Ouvrir » à la première recherche (Cmd+P, `note 009999`) qui renvoie le bon résultat, ce qui prouve que l'index est complet.

## Valeurs mesurées

### Writer (référence)

| Mesure | macOS | Statut |
|---|---|---|
| Démarrage à froid | à mesurer | US-046 (EP-010) |
| Démarrage à chaud | à mesurer | US-046 (EP-010) |
| Mémoire, 10 000 fichiers | à mesurer | US-046 (EP-010) |
| Ouverture, 10 000 fichiers | à mesurer | US-046 (EP-010) |

Writer ne tourne pas sous Windows ni sous Linux : ces colonnes n'existent pas pour lui.

### Cerata

| Mesure | Windows | macOS | Linux | Commande |
|---|---|---|---|---|
| Indexation, 10 000 fichiers (cache disque chaud) | médiane 32 ms (min 30, max 33, 7 runs) | US-046 | US-045 | `index_bench index <ws10k> 7` |
| Indexation, 50 000 fichiers (cache disque chaud) | médiane 457 ms (min 379, max 492, 5 runs) | US-046 | US-045 | `index_bench index <ws50k> 5` |
| Éditeur, 1 Mo : ouverture jusqu'à la première frame présentée | 24,4 à 25,5 ms (3 runs) | US-046 | US-045 | `cargo run -p cerata --example editor_bench --release` |
| Éditeur, 1 Mo : frappe, travail de l'application (frame vers présentée) | p99 2,13 à 2,18 ms, aucune frappe hors de la frame suivante (3 × 200 frappes) | US-046 | US-045 | idem |
| Éditeur, 1 Mo : frappe, gestionnaire d'entrée seul | p99 3,27 à 3,74 ms | US-046 | US-045 | idem |
| Éditeur, 1 Mo : saut de défilement à froid, travail de la frame | p99 4,26 à 4,81 ms (3 × 60 sauts) | US-046 | US-045 | idem |
| Éditeur, tableau de 200 lignes × 8 colonnes : frappe dans une cellule, travail de l'application (frame vers présentée) | p99 1,24 à 1,64 ms, max 2,3 ms, aucune frappe hors de la frame suivante (3 × 200 frappes) | US-046 | US-045 | idem |
| Éditeur, tableau de 200 lignes × 8 colonnes : frappe dans une cellule, gestionnaire d'entrée seul | p99 1,28 à 1,51 ms | US-046 | US-045 | idem |
| Éditeur, tableau de 200 lignes × 8 colonnes : « Insert column right », travail de l'application | p99 9,38 à 11,66 ms, aucune commande hors de la frame suivante (3 × 60 insertions) | US-046 | US-045 | idem |
| Éditeur, tableau de 1 000 lignes × 8 colonnes : frappe dans la dernière cellule, travail de l'application | p99 6,25 à 9,19 ms, max 11,0 ms, aucune frappe hors de la frame suivante (4 × 200 frappes, la quatrième après les correctifs de la revue d'interface) | US-046 | US-045 | idem |
| Éditeur, tableau de 1 000 lignes × 8 colonnes : frappe dans la dernière cellule, gestionnaire d'entrée seul | p99 4,99 à 6,21 ms | US-046 | US-045 | idem |
| Sidebar, dossier plat de 50 000 notes : lancement jusqu'à l'arbre affiché (cache disque chaud) | médiane 486 ms (min 450, max 524, 5 runs), indexation comprise | US-046 | US-045 | mesure manuelle ci-dessous |
| Sidebar, 50 000 lignes : défilement à la molette, travail de la frame | p99 1,26 à 1,32 ms, max 1,71 ms, aucune frame au-delà de 16,7 ms (5 × 300 crans, sidebar reconstruite) | US-046 | US-045 | `ZED_MEASUREMENTS=1`, mesure manuelle ci-dessous |
| Sidebar, 50 000 lignes : reconstruction de l'arbre pendant le remplissage des titres | 10 à 13 reconstructions de 39 à 71 ms sur le thread de l'interface | US-046 | US-045 | idem |
| Démarrage, mémoire | sans objet tant que l'interface n'existe pas (EP-004) | | | |

Mesures Windows du 2026-10-31, build release, workspace généré dans le dossier temporaire de l'utilisateur sur le SSD NVMe. Les valeurs à froid demandent un redémarrage et n'ont pas encore été prises.

Mesures de l'éditeur (EP-003) du 2026-11-01, build release, fixture de 1 Mo du spike (`fixture::generate`). `editor_bench` ouvre la note dans une fenêtre déjà affichée, tape 200 caractères par `EntityInputHandler::replace_text_in_range` à une phase aléatoire du rafraîchissement, puis fait 60 sauts de 2,5 hauteurs de vue. La métrique est celle de l'ADR 0001 (attente du vsync exclue) ; le coût du gestionnaire d'entrée y est isolé.

L'éditeur a été remesuré le 2026-11-01 à la revue d'EP-005, même fixture et même procédure. La première mesure de l'epic donnait une frappe à p99 17,9 à 18,4 ms et un défilement à p99 13,4 à 14,1 ms : le rail des sections construisait un repère interactif par titre à chaque frame (environ 1 800 sur la fixture) et relisait les titres du document entier à chaque frappe. Le rail ne construit plus que les repères visibles, et les titres sont relus 150 ms après la dernière frappe, comme dans Writer (`use-document-headings.ts:85-87`). Les valeurs EP-003 (ouverture 18,6 à 21,0 ms, frappe p99 1,50 à 1,68 ms, gestionnaire p99 1,88 à 2,14 ms, défilement p99 3,55 à 4,02 ms) sont remplacées. L'écart qui reste, surtout sur le gestionnaire d'entrée (+1,4 ms), n'a pas été attribué ; il tient dans le budget de 16 ms.

Le scénario du tableau (US-048) a été ajouté le 2026-11-02 : `editor_bench` ouvre une note qui ne contient qu'un tableau de 200 lignes et 8 colonnes, place le caret à la fin de la première cellule de la ligne 100 et y tape 200 caractères par le même gestionnaire d'entrée. Le rapport vérifie que le caret reste dans une cellule dessinée et que la ligne garde ses 8 cellules. La première mesure donnait un p99 de 35,1 ms : chaque frappe remettait en page tout le tableau, et chaque cellule parcourait toutes les décorations et tous les styles du tableau. Le repli retenu (ADR 0007) garde la mise en page du tableau par état de l'éditeur et cherche les entrées de chaque cellule par recherche dichotomique. Les mesures du document de 1 Mo prises dans la même session (frappe p99 2,19 à 2,38 ms, défilement p99 4,00 à 4,14 ms, ouverture 24,0 à 26,5 ms) restent dans l'écart des valeurs ci-dessus.

Les scénarios d'EP-007 (US-057) ont été ajoutés le 2026-11-02. Depuis la cellule du tableau de 200 lignes où il vient de taper, `editor_bench` lance 60 fois la commande `table.columnRight` du registre, chaque insertion annulée hors chronométrage pour que toutes s'appliquent au même tableau de 8 colonnes ; le rapport vérifie qu'il en reste 8 à la fin. Il ouvre ensuite une note de 1 000 lignes × 8 colonnes, place le caret à la fin de la dernière cellule et y tape 200 caractères ; le rapport vérifie que le texte tapé précède le caret et que le caret reste dans une cellule dessinée. La première mesure du grand tableau donnait un p99 de 72,9 ms : chaque frappe remettait en page les 8 000 cellules, chaque ligne repliée du tableau filtrait toutes les décorations de la frame (coût quadratique), la frame décorait tout le contenu du tableau et la grille était dérivée à nouveau à chaque appel. Les décorations de la frame sont maintenant indexées par début, la frame laisse de côté le contenu des tableaux dessinés, les dernières grilles dérivées sont gardées par texte et position de leurs lignes, et une frappe dans un tableau ne remet en page que les cellules qu'elle touche (repli « cache des cellules inchangées » de l'ADR 0007) : les autres reprennent leur ligne mise en forme de l'état précédent. Les mêmes changements font passer la frappe dans le tableau de 200 lignes de p99 4,28 à 4,99 ms à p99 1,24 à 1,64 ms ; les valeurs du document de 1 Mo prises dans la même session (frappe p99 2,14 à 2,31 ms, gestionnaire p99 3,32 à 3,46 ms, défilement p99 3,88 à 4,47 ms, ouverture 23,0 à 23,4 ms) restent dans l'écart des valeurs ci-dessus.

Mesures de la sidebar (US-026) du 2026-11-01, build release, sur la machine Windows de référence. Le corpus est un cas limite : 50 000 notes `note NNNNNN.md` (titre `# Note N`) dans un seul dossier, à la racine du workspace, pour que l'arbre affiche 50 000 lignes sans dépliage ; le corpus du générateur n'affiche jamais plus de 100 lignes par dossier. Le binaire est lancé avec `ZED_MEASUREMENTS=1`, qui fait journaliser par GPUI la durée de chaque frame dessinée (`gpui_util::measure`, attente du vsync exclue). Les 300 crans de molette sont envoyés par `SendInput` à 16 ms d'intervalle, le pointeur sur l'arbre, une fois les titres remplis. Le délai jusqu'à l'arbre va du lancement du processus à la première reconstruction des lignes (journal temporaire, retiré depuis).

Le défilement a été remesuré le 2026-11-01 après la reconstruction de la sidebar sur les mesures de Paneflow (lignes, animations d'ouverture des sections et des dossiers, filtre en place) : même corpus, même procédure, 300 frames journalisées par run pour 300 crans (`docs/captures/ep-004/us-026-arbre-50k-revue2.png`). Les valeurs de la première mesure (p99 1,50 à 2,49 ms) sont remplacées.

L'arbre s'affiche dès que le dossier est listé, par nom de fichier, et les titres arrivent ensuite par lots qui doublent à partir de 512. Avant ce correctif, la sidebar lisait les 50 000 titres et faisait deux appels `fs::metadata` par fichier avant d'afficher quoi que ce soit : l'arbre restait vide 18 s à chaud et plus d'une minute au premier lancement. La lecture des titres dépend ensuite du système : entre 2 et 30 s pour les 50 000 selon les runs, avec des lots de 8 000 fichiers qui prennent jusqu'à 30 s (l'analyse antivirus des fichiers ouverts est l'explication probable ; elle n'a pas été isolée). L'arbre reste visible et défile pendant ce temps.
