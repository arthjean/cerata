# Benchmarks et machines de référence

Ce document fixe les machines sur lesquelles Cerata est mesuré et la procédure qui mesure Writer (writer-computer au commit `79b744c`) pour lui servir de point de comparaison. Toute valeur citée dans une ADR ou une story vient d'une de ces machines, avec la commande qui l'a produite.

## Machines de référence

| OS | Machine | Processeur | Mémoire | GPU | Stockage | Système | Statut |
|---|---|---|---|---|---|---|---|
| Windows | poste fixe d'Arthur | AMD Ryzen 7 7800X3D (8 cœurs) | 32 Go | NVIDIA RTX 4070 Ti SUPER (iGPU AMD Radeon présent) | Samsung SSD 990 PRO 1 To (NVMe) | Windows 11 Pro 10.0.26300 | désignée le 2026-10-31 |
| macOS | Mac Apple Silicon loué chez Scaleway (seule cible de Writer) | | | | | | à documenter en US-046 (EP-008) |
| Linux | machine d'Arthur (Wayland et X11 sur la même machine) | | | | | | à documenter en US-045 (EP-007) |

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
| Démarrage à froid | à mesurer | US-046 (EP-008) |
| Démarrage à chaud | à mesurer | US-046 (EP-008) |
| Mémoire, 10 000 fichiers | à mesurer | US-046 (EP-008) |
| Ouverture, 10 000 fichiers | à mesurer | US-046 (EP-008) |

Writer ne tourne pas sous Windows ni sous Linux : ces colonnes n'existent pas pour lui.

### Cerata

| Mesure | Windows | macOS | Linux | Commande |
|---|---|---|---|---|
| Indexation, 10 000 fichiers (cache disque chaud) | médiane 32 ms (min 30, max 33, 7 runs) | US-046 | US-045 | `index_bench index <ws10k> 7` |
| Indexation, 50 000 fichiers (cache disque chaud) | médiane 457 ms (min 379, max 492, 5 runs) | US-046 | US-045 | `index_bench index <ws50k> 5` |
| Éditeur, 1 Mo : ouverture jusqu'à la première frame présentée | 18,6 à 21,0 ms (5 runs) | US-046 | US-045 | `cargo run -p cerata --example editor_bench --release` |
| Éditeur, 1 Mo : frappe, travail de l'application (frame vers présentée) | p99 1,50 à 1,68 ms, aucune frappe hors de la frame suivante (5 × 200 frappes) | US-046 | US-045 | idem |
| Éditeur, 1 Mo : frappe, gestionnaire d'entrée seul | p99 1,88 à 2,14 ms | US-046 | US-045 | idem |
| Éditeur, 1 Mo : saut de défilement à froid, travail de la frame | p99 3,55 à 4,02 ms (5 × 60 sauts) | US-046 | US-045 | idem |
| Sidebar, dossier plat de 50 000 notes : lancement jusqu'à l'arbre affiché (cache disque chaud) | médiane 486 ms (min 450, max 524, 5 runs), indexation comprise | US-046 | US-045 | mesure manuelle ci-dessous |
| Sidebar, 50 000 lignes : défilement à la molette, travail de la frame | p99 1,50 à 2,49 ms, max 2,55 ms, aucune frame au-delà de 16,7 ms (5 × 300 crans) | US-046 | US-045 | `ZED_MEASUREMENTS=1`, mesure manuelle ci-dessous |
| Sidebar, 50 000 lignes : reconstruction de l'arbre pendant le remplissage des titres | 10 à 13 reconstructions de 39 à 71 ms sur le thread de l'interface | US-046 | US-045 | idem |
| Démarrage, mémoire | sans objet tant que l'interface n'existe pas (EP-004) | | | |

Mesures Windows du 2026-10-31, build release, workspace généré dans le dossier temporaire de l'utilisateur sur le SSD NVMe. Les valeurs à froid demandent un redémarrage et n'ont pas encore été prises.

Mesures de l'éditeur (EP-003) du 2026-11-01, build release, fixture de 1 Mo du spike (`fixture::generate`). `editor_bench` ouvre la note dans une fenêtre déjà affichée, tape 200 caractères par `EntityInputHandler::replace_text_in_range` à une phase aléatoire du rafraîchissement, puis fait 60 sauts de 2,5 hauteurs de vue. La métrique est celle de l'ADR 0001 (attente du vsync exclue) ; le coût du gestionnaire d'entrée y est isolé.

Mesures de la sidebar (US-026) du 2026-11-01, build release, sur la machine Windows de référence. Le corpus est un cas limite : 50 000 notes `note NNNNNN.md` (titre `# Note N`) dans un seul dossier, à la racine du workspace, pour que l'arbre affiche 50 000 lignes sans dépliage ; le corpus du générateur n'affiche jamais plus de 100 lignes par dossier. Le binaire est lancé avec `ZED_MEASUREMENTS=1`, qui fait journaliser par GPUI la durée de chaque frame dessinée (`gpui_util::measure`, attente du vsync exclue). Les 300 crans de molette sont envoyés par `SendInput` à 16 ms d'intervalle, le pointeur sur l'arbre, une fois les titres remplis. Le délai jusqu'à l'arbre va du lancement du processus à la première reconstruction des lignes (journal temporaire, retiré depuis).

L'arbre s'affiche dès que le dossier est listé, par nom de fichier, et les titres arrivent ensuite par lots qui doublent à partir de 512. Avant ce correctif, la sidebar lisait les 50 000 titres et faisait deux appels `fs::metadata` par fichier avant d'afficher quoi que ce soit : l'arbre restait vide 18 s à chaud et plus d'une minute au premier lancement. La lecture des titres dépend ensuite du système : entre 2 et 30 s pour les 50 000 selon les runs, avec des lots de 8 000 fichiers qui prennent jusqu'à 30 s (l'analyse antivirus des fichiers ouverts est l'explication probable ; elle n'a pas été isolée). L'arbre reste visible et défile pendant ce temps.
