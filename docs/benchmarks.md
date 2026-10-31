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
| Démarrage, mémoire | sans objet tant que l'interface n'existe pas (EP-004) | | | |

Mesures Windows du 2026-10-31, build release, workspace généré dans le dossier temporaire de l'utilisateur sur le SSD NVMe. Les valeurs à froid demandent un redémarrage et n'ont pas encore été prises.
