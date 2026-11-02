# Images à la Notion : glisser-déposer, import, redimensionnement, barre d'outils

> Statut : recherche vérifiée, **rien d'implémenté**. Rédigée le 2026-11-02.
> Base : `main` au commit `3bb7550` ; GPUI à la rev épinglée `37faef8` (checkout `~/.cargo/git/checkouts/zed-a70e2ad075855582/37faef8`).
> Suite attendue : une ADR 0008 (format de la largeur et sélection d'image, qui amende l'ADR 0006), puis une epic dans le PRD.

## 0. Verdict

- **Le rendu et le collage existent déjà** (US-020) : `![alt](url)`, `![[embed]]`, `<img>` en bloc HTML, collage d'une image du presse-papiers dans `{stem}-assets/`. Tout le reste est à construire : dépôt dans l'éditeur, import par sélecteur, largeur persistée, poignées, barre d'outils.
- **Format recommandé : la syntaxe Obsidian**, `![alt|300](chemin)` et `![[image.png|300]]`. Elle est documentée par Obsidian, déjà tolérée par le parseur de Cerata (qui jette le suffixe aujourd'hui), et se dégrade proprement ailleurs : un rendu CommonMark affiche l'image en taille naturelle, avec `alt|300` pour texte alternatif.
- **Un seul pipeline d'insertion** pour le dépôt, l'import et le collage. Il valide par signature d'octets, écrit par `WorkspaceFs` hors du thread principal, et insère un bloc image par ligne, en une seule étape d'annulation.
- **Poignées et barre d'outils en enfants GPUI de l'élément image**, visibles au survol. Le glisser passe par la machinerie `Drag` existante de l'éditeur. Pendant le geste, la largeur n'existe que dans l'état de la vue ; au relâchement, une seule transaction réécrit le texte alternatif.
- **Une décision d'UX conditionne le ressenti Notion.** Aujourd'hui, un clic sur une image révèle sa source au-dessus d'elle, et l'image saute d'une ligne sous le pointeur. Il faut qu'une sélection exactement égale au nœud garde l'image dessinée, avec un anneau de sélection, comme l'ADR 0007 l'a fait pour les tableaux.
- **Limite GPUI non contournable sans fork** : sous Windows, le dépôt n'accepte que `CF_HDROP`. Une image glissée depuis un navigateur est donc ignorée ; le copier-coller depuis le navigateur fonctionne.

## 1. Méthode et vérification

Cinq sous-agents en parallèle, puis vérification de chaque affirmation structurante dans le code ou la source primaire.

| Périmètre | Vérifié par moi | Corrections |
|---|---|---|
| Pipeline image de l'éditeur | `decorate.rs:708-735`, `wiki.rs:76-84` et test `:163`, `html.rs:371-397`, `images.rs:487-527` et `:588-630`, `actions.rs:762-822`, `mouse.rs:29-46` et `:266-303`, `element.rs:698-709` et `:814-828`, `mod.rs:313-316` et `:382-399`, `history.rs:34-40` et `:95-112` | aucune |
| Shell et couche fichiers | `shell/mod.rs:1637-1654`, `routing.rs:35-47`, `cerata_core/src/fs/mod.rs:118-134`, `:186-194` et `:227-235`, `watcher.rs:356-381` | aucune |
| GPUI 37faef8 | `interactive.rs:757-780`, `window.rs:3407-3429` et `:5726-5775`, `div.rs:338`, `:360`, `:592`, `:2962-2993`, `platform.rs:2919-2928` et `:3338-3393`, `gpui_windows/src/window.rs:1160-1210`, `gpui_windows/src/clipboard.rs:28-47` et `:112-117` | précision : `on_drop` consomme le drag **avant** d'évaluer `can_drop` (voir §3) |
| writer-computer et Paneflow | `SPECs/obsidian-image-embed-spec.md:19` (taille hors périmètre v1), pas de dépôt dans l'éditeur | aucune |
| Syntaxes et UX Notion (web) | dépôt `obsidianmd/obsidian-help` (sources Markdown des pages d'aide), page d'aide Notion | **l'agent affirmait que `![alt\|300](url)` n'est pas documenté par Obsidian : faux** (voir §4) |

Non vérifié directement : les backends macOS et Linux du dépôt et du presse-papiers, le comportement exact d'Obsidian pour `|LxH` (hauteur), et la syntaxe exportée par Notion (non documentée).

## 2. État actuel de Cerata

| Brique | Où | Constat |
|---|---|---|
| Parsing image Markdown | `cerata_markdown/src/tree/inline.rs:720`, nœud `Image` | Le texte alternatif n'a pas de nœud propre : il couvre la plage entre `![` et `]` |
| Décoration | `decorate.rs:708-735` (`fold_image`) | Lit seulement l'`Url`. Si le caret est dans la destination, rien n'est dessiné ; si la sélection touche le nœud, un widget est placé après la source ; sinon la source est remplacée. L'image est un bloc si elle remplit sa ligne. `Widget::Image { url }` ne porte aucune taille |
| Embed `![[x.png\|400]]` | `wiki.rs:76-84` | Le suffixe après `\|` est parsé puis jeté (test `"pic.png\|400" -> "pic.png"`, `wiki.rs:163`) |
| `<img width>` en bloc HTML | `editor/html.rs:381-385` | Seule largeur honorée aujourd'hui, comme maximum borné à la colonne, et passée comme clé du cache de hauteurs |
| Taille affichée | `images.rs:513` | `display = min(naturelle, colonne)` : impossible d'agrandir au-delà de la taille naturelle |
| Position | `element.rs:698` | `origin = column_left` : les blocs image sont alignés à gauche |
| Colonne | `config.rs:41`, `mod.rs:313-316` | 734 px par défaut, réglable ; le zoom agrandit le texte, pas les images |
| Clic sur une image | `mouse.rs:295-303` | Sélectionne toute la plage du nœud, ancrée à la fin. La décoration voit le nœud « touché » et révèle la source |
| Interaction de glisser | `mouse.rs:29-46` (`Drag::Thumb`), `element.rs:814-827` | Pendant un `Drag`, l'élément installe des écouteurs `MouseMove`/`MouseUp` à l'échelle de la fenêtre. C'est le modèle à suivre pour les poignées |
| Overlays | `complete.rs:261`, `menu.rs:291` | `deferred(anchored())` pour la complétion et le menu contextuel |
| Éditions | `mod.rs:382-399` (`update_state`), `state.rs` (`Transaction`), `history.rs:34-40` | `UserEvent::Command` ne fusionne jamais : une transaction donne une étape d'annulation. `ChangeSet::map_pos` existe (`change.rs:127`) |
| Collage | `actions.rs:762-822`, `images.rs:588-630` | Synchrone sur le thread principal. Limite de 5 Mo, `{stem}-assets/AAAAMMJJ-HHMMSS-xxxx.ext`, `atomic_write`. Insertion **en ligne** au caret de `![image.ext](dest)`. Ignore les entrées `ExternalPaths` (fichier copié dans l'Explorateur) |
| Confinement | `images.rs:605` et `:624` | `std::fs::create_dir_all` et `atomic_write` directs, sans passer par `WorkspaceFs::write_bytes` (`fs/mod.rs:227-235`). Le PRD exige 100 % d'opérations confinées (`prd-cerata-port.md:1573`) |
| Dépôt de fichiers | `shell/mod.rs:1637-1654`, `routing.rs:35-47` | Seul `on_drop` de l'app, sur la racine du shell. Il ouvre les dossiers et les `.md` ; une image déposée affiche « Only folders and Markdown files can be opened » |
| Sidebar et watcher | `fs/mod.rs:186-194`, `watcher.rs:376-381` | Seuls dossiers et `.md` sont listés ; un dossier `-assets` sans Markdown est caché. Un fichier non Markdown ne produit jamais de `FileChanged` |
| Renommer ou déplacer une note | `fs/mod.rs:310-328` | Le dossier `{stem}-assets` ne suit pas, et aucun lien n'est réécrit (`links.rs` ne fait que résoudre) |
| Sélecteur de fichiers | `shell/commands.rs:196-236` | `prompt_for_paths` existe pour l'espace de travail et les fichiers, pas pour les images |
| Menu « / » | aucun | Pas de menu d'insertion de blocs |

## 3. Ce que GPUI 37faef8 permet

**Dépôt externe.** `FileDropEvent` (`interactive.rs:757-780`) est traduit en événements souris (`window.rs:5726-5775`) : `Entered` crée un drag `ExternalPaths` puis un `MouseMove` bouton pressé, `Pending` devient un `MouseMove`, `Submit` un `MouseUp`. Une `div` reçoit le survol par `on_drag_move::<ExternalPaths>` (`div.rs:360`), le dépôt par `on_drop` (`div.rs:592`), et la sortie par `on_file_drop_exit` (`div.rs:338`). La position se lit dans l'événement ou dans `window.mouse_position()` (`window.rs:3407`).

**Piège vérifié.** Le listener de dépôt fait `cx.active_drag.take()` avant d'appeler le prédicat `can_drop` (`div.rs:2972-2986`). Un `can_drop` qui refuse fait donc perdre le drag, et le handler du shell ne le voit jamais. Conséquence : le handler de l'éditeur doit tout traiter, en gardant les images et en renvoyant le reste (`.md`, dossiers) vers la route existante du shell.

**Windows.** `IDropTarget` n'interroge que `CF_HDROP`/`TYMED_HGLOBAL` et répond `DROPEFFECT_NONE` sinon (`gpui_windows/src/window.rs:1167-1206`). Une image glissée depuis un navigateur n'arrive donc pas.

**Presse-papiers.** `ClipboardEntry` vaut `String`, `Image` ou `ExternalPaths`. Sous Windows, les formats lus sont PNG, GIF, JFIF, `image/svg+xml` et `CF_DIB` converti en BMP (`clipboard.rs:28-47`) ; un fichier copié dans l'Explorateur arrive en `ExternalPaths` via `CF_HDROP` (`clipboard.rs:115-117`). `Image::from_bytes` calcule `id` comme un hachage des octets (`platform.rs:3387-3393`).

**Glisser interne.** `window.capture_pointer(hitbox_id)` route les `MouseMove`/`MouseUp` hors des bornes et se libère au relâchement (`window.rs:3411-3429`). `set_window_cursor_style` maintient le curseur pendant un geste. `CursorStyle::ResizeLeftRight` est déjà utilisé par le shell (`shell/mod.rs:1873`).

**Sélecteur.** `PathPromptOptions` n'a que `files`, `directories`, `multiple`, `prompt` (`platform.rs:2919-2928`) : aucun filtre par extension, donc on filtre après coup.

**Rendu.** `img()` met à l'échelle sur le GPU l'image décodée une fois, avec `ObjectFit::Contain` par défaut, et anime les GIF (`elements/img.rs`). Rapport d'agent, non relu ligne à ligne : ce n'est pas bloquant.

## 4. Où persister la largeur

| Forme | Source primaire | Dans Obsidian | Ailleurs (CommonMark, GitHub, VS Code) |
|---|---|---|---|
| `![alt\|300](x.png)`, `![alt\|300x200](x.png)` | `obsidian-help`, *Basic formatting syntax* : « You can change the image dimensions, by adding `\|640x480` to the link destination […] If you only specify the width, the image scales according to its original aspect ratio » | largeur honorée | image en taille naturelle, alt `alt\|300` : dégradation propre |
| `![300](https://…)` (alt numérique seul) | `obsidian-help`, *Embed files* : « You can control the width and height using the same syntax as a wikilink » | largeur honorée | idem |
| `![[x.png\|300]]` | `obsidian-help`, *Embed files* | largeur honorée | texte littéral (déjà le cas de tout `![[…]]`) |
| `<img src="x.png" width="300">` | CommonMark 0.31.2 (HTML brut transmis) | honoré | honoré partout où le HTML brut passe |
| `![](x.png){width=300}` | Pandoc, extension `link_attributes` | texte parasite | texte parasite hors Pandoc |

CommonMark ne définit aucune taille d'image (spec 0.31.2, §6.4).

**Recommandation : la forme Obsidian.**
- Cerata lit déjà les coffres Obsidian (`![[embed]]`, résolution des liens) et parse déjà le `|`.
- La forme se dégrade sans casser le rendu ailleurs.
- Réécrire une image Markdown en `<img>` au premier redimensionnement la ferait basculer dans le moteur de blocs HTML (`html.rs`, liste blanche, autre règle de révélation), et changerait le texte de l'utilisateur bien plus qu'un suffixe.

Règles proposées :

- **Lecture.** On découpe l'alt au dernier `|` non échappé. Si le suffixe correspond à `^\d+(x\d+)?$`, c'est une taille. Sinon, si l'alt entier correspond, c'est une taille avec un alt vide (forme `![300](…)`). Seule la largeur est appliquée, le ratio étant conservé. Le comportement d'Obsidian pour la hauteur n'est pas vérifié.
- **Écriture.** Le redimensionnement écrit `alt|W` et retire toute hauteur. Il retire le suffixe quand la largeur revient à la largeur par défaut, `min(naturelle, colonne)`, avec un aimant de quelques pixels. Le fichier reste propre.
- **Unité.** Des pixels logiques, comme les tailles naturelles aujourd'hui (le zoom n'agit pas sur les images). C'est ce qu'écrit Obsidian.
- **Bornes à l'affichage.** `min(W, colonne)` et un minimum fixe (par exemple 64 px). L'agrandissement au-delà de la taille naturelle est permis jusqu'à la colonne, comme dans Notion.

## 5. Approche recommandée

### 5.1 Modèle et rendu

- **`cerata_markdown`.** Une fonction pure `image_size(alt) -> (plage_alt_sans_suffixe, Option<largeur>)`, et `Widget::Image { url, width }` / `Widget::WikiImage { …, width }`. `image_embed_target` renvoie aussi la taille au lieu de la jeter. Une commande pure `set_image_width(state, nœud, Option<u32>) -> Change` est testable comme `table_ops`.
- **`ImageHost::sized`.** On sépare la largeur demandée de la borne de colonne : `display = min(demandée.unwrap_or(naturelle), colonne)`. La clé du cache de hauteurs reste la largeur passée, comme pour `<img width>` (`html.rs:385`), donc aucun saut de hauteur.
- **Alignement.** Les blocs image plus étroits que la colonne sont centrés : `origin.x = column_left + (colonne - largeur) / 2` à `element.rs:698`, comme Notion sur les captures. Un redimensionnement centré est symétrique : chaque poignée change la largeur de `2 × dx`. Les images en ligne ne changent pas.

### 5.2 Sélection et révélation (amende l'ADR 0006, à acter en ADR 0008)

Une plage de sélection **exactement égale** au nœud image est une sélection d'objet. La décoration garde alors `Replace` et l'image est peinte avec un anneau de sélection. Toute autre plage qui touche le nœud garde la règle actuelle (source et widget dessous), et le clavier entre toujours dans la source par les flèches.

Effets :
- Le clic (`mouse.rs:295-303`) ne fait plus sauter l'image.
- `Backspace` et `Suppr` suppriment l'image, en une étape annulable. Le fichier reste sur le disque.
- Copier copie le Markdown.

C'est le même mouvement que l'ADR 0007 pour les tableaux, et le changement est local à `fold_image` et `fold_wiki_link`.

### 5.3 Redimensionnement

Les poignées sont des enfants de l'élément image : deux barres verticales arrondies à mi-hauteur, comme sur les captures. Elles apparaissent au survol (`group_hover`), avec le curseur `ResizeLeftRight`. Leur `on_mouse_down` appelle `stop_propagation`, comme les `<details>` HTML (`html.rs:~338`), pour que le clic ne sélectionne pas l'image. Elles posent `Drag::ImageResize { range, start_x, start_width, côté }` dans l'éditeur, à côté de `Drag::Thumb`.

Pendant le geste :
- Les écouteurs fenêtre existants (`element.rs:814-827`) mettent à jour `resize_preview` dans la vue, et la mise en page l'utilise pour cette ligne.
- Le buffer n'est pas touché.
- `capture_pointer` et `set_window_cursor_style(ResizeLeftRight)` tiennent le geste hors des bornes.

Au relâchement :
- On vérifie que la plage contient toujours la même source, car un rechargement disque peut survenir pendant le geste.
- Puis une seule `Transaction` `UserEvent::Command` réécrit l'alt, soit une étape d'annulation.
- `Échap` pendant le geste annule.

Les tests existants qui simulent un glisser (`tests.rs`, `drag_across_…`) servent de gabarit.

### 5.4 Barre d'outils au survol

C'est un enfant positionné en haut à droite de l'élément image, visible au survol comme les poignées. Placée à l'intérieur des bornes de l'image, elle ne laisse aucun espace mort entre l'image et la barre, donc aucune logique de survol à temporiser. Elle n'a pas besoin de `deferred(anchored())`.

Actions v1, équivalents locaux de Notion :

| Action | Détail |
|---|---|
| Taille d'origine | retire le suffixe ; visible seulement si une largeur est posée |
| Copier l'image | `ClipboardEntry::Image` |
| Ouvrir | `cx.open_with_system`, déjà utilisé par `links.rs:185` |
| Modifier la source | caret dans l'alt |
| Supprimer | retire le Markdown, jamais le fichier |

Ces actions passent aussi par le menu contextuel de l'éditeur (`editor/menu.rs`) quand le clic droit tombe sur une image, pour le clavier et l'accessibilité.

Sont exclus, faute de représentation Markdown portable ou parce qu'ils relèvent d'un autre chantier : commentaire, IA, recadrage, alignement gauche ou droite, légende, plein écran.

### 5.5 Pipeline d'insertion unique

Entrées :
- fichiers déposés (`ExternalPaths`) ;
- fichiers choisis au sélecteur ;
- presse-papiers : `ExternalPaths` d'abord (fichier copié dans l'Explorateur, ordre de Paneflow `src-app/src/terminal/input.rs:1104-1137`), puis `Image`, puis texte.

Étapes :

1. **Note sans chemin** : refus avec la notice `NoFolder` existante.
2. **Validation par signature** (`sniff_format`, `images.rs:172`), jamais par extension. Les non-images sont signalées dans une notice ; dans un dépôt mixte, les `.md` et les dossiers sont renvoyés à la route du shell (§3).
3. **Destination :**
   - un fichier déjà dans la racine de l'espace de travail est lié par chemin relatif, sans copie ;
   - sinon, il est copié dans `{stem}-assets/` sous son nom d'origine assaini, avec un suffixe ` 1`, ` 2` en cas de collision, et son nom sans extension sert d'alt ;
   - des octets collés gardent le nom horodaté actuel.
4. **Écriture** par `WorkspaceFs::write_bytes`, ce qui corrige le contournement actuel du confinement, hors du thread principal. Le modèle à reprendre est `spawn_blocking_then` de Paneflow (`src-app/src/app/diff_dock/code/mod.rs:22-36`).
5. **Insertion** au point cible, ramené par `ChangeSet::map_pos` si l'utilisateur a tapé entre-temps :
   - une image par ligne ;
   - une ligne vide avant et après si les lignes voisines ne sont pas vides, pour que le fichier reste correct en CommonMark ;
   - une seule transaction pour tout le lot.
6. **Limite.** On garde 5 Mo pour les octets collés (critère d'US-020). Pour une copie de fichier, la contrainte d'IPC de Writer n'existe pas : le plafond de pixels du rendu (`MAX_PIXELS`, `images.rs:32`) suffit. C'est une décision à prendre (§8).

Pour le dépôt dans l'éditeur, on ajoute sur la `div` racine (`mod.rs:927-938`) :
- `on_drag_move::<ExternalPaths>` : la position est convertie en frontière de ligne, stockée dans `drop_target`, et l'élément peint une barre d'insertion horizontale entre deux blocs, comme Notion ;
- `on_file_drop_exit` : efface la cible ;
- `on_drop` : lance le pipeline.

Le handler du shell reste pour les dépôts hors de l'éditeur. Son corps devient une fonction partagée pour le renvoi depuis l'éditeur.

Pour l'import, une commande de palette « Insérer une image… » est disponible quand un fichier est actif, avec une entrée au menu contextuel. Elle appelle `prompt_for_paths { files: true, directories: false, multiple: true }`. Le menu « / » est un chantier séparé.

## 6. Rejeté

- **`<img width>` comme format d'écriture** : il change la nature du bloc et le texte de l'utilisateur (§4).
- **Largeur relative (pourcentage)** : Obsidian ne la lit pas. Notion n'en documente pas l'export (page d'export muette sur les images).
- **Patch de GPUI pour les images glissées depuis un navigateur** : la politique du dépôt interdit tout fork (`Cargo.toml`, commentaire de l'épinglage).
- **Image rendue comme bloc atomique au sens de l'ADR 0006** : l'atomicité sert les fences Mermaid (`atomic.rs:51-63`). La sélection d'objet du §5.2 suffit et garde la source accessible au clavier.

## 7. Risques et points ouverts

- **Pré-existant, aggravé par la fonctionnalité** : renommer, déplacer ou dupliquer une note laisse `{stem}-assets` derrière, et les images cassent (`fs/mod.rs:310-358`). Avec le dépôt et l'import, les images deviennent courantes. Faire suivre le dossier dans `WorkspaceFs::rename`/`duplicate` mérite une story propre.
- **Hauteur `|LxH` d'Obsidian** : rendu exact non vérifié. Proposition : lire, ignorer la hauteur, ne jamais l'écrire.
- **Ambiguïté de `![2024](x.png)`** : un alt purement numérique devient une largeur, comme dans Obsidian. La compatibilité prime.
- **macOS et Linux** : dépôt et presse-papiers non relus dans les backends ; à couvrir par EP-009 et EP-010.
- **Simulation de dépôt en test** : les tests du shell appellent directement `view.on_drop(&ExternalPaths(…))` (`shell/tests.rs:221`). Je n'ai trouvé aucun helper GPUI de simulation de `FileDropEvent` dans `TestAppContext`. Le pipeline doit donc être testable sans l'événement plateforme.

## 8. Décisions à prendre

1. **Format** : forme Obsidian `|W` (recommandé) ou `<img width>`.
2. **Alignement** : centrer les blocs image plus étroits que la colonne, avec redimensionnement symétrique (recommandé, conforme aux captures), ou garder l'alignement à gauche, avec une poignée droite seule.
3. **Sélection d'objet** (§5.2) : amender l'ADR 0006 pour les images (recommandé).
4. **Collage** : insérer en bloc sur sa propre ligne, comme le dépôt et l'import (recommandé), ou garder l'insertion en ligne actuelle.
5. **Fichier déjà dans l'espace de travail** : lier sans copier (recommandé) ou toujours copier.
6. **Taille des fichiers déposés ou importés** : pas de limite en octets (recommandé), ou les 5 Mo du collage.

## 9. Découpage proposé

| Story | Contenu | Tests |
|---|---|---|
| A. Largeur et sélection d'objet | §4, §5.1, §5.2 ; ADR 0008 | `cerata_markdown` : lecture et écriture du suffixe (toutes les formes du §4, échappement `\|`), décoration en sélection exacte ; éditeur : hauteur au redimensionnement, clic sans saut |
| B. Poignées | §5.3 | glisser simulé sur la poignée (`debug_selector`), une étape d'annulation, aimant vers la taille d'origine, `Échap` |
| C. Pipeline d'insertion | §5.5 : dépôt, sélecteur, `ExternalPaths` collé, écriture confinée et asynchrone | lot mixte `.png` + `.md`, collision de noms, fichier interne lié sans copie, note sans chemin, non-image refusée |
| D. Barre d'outils et menu | §5.4 | chaque action, accessibilité, revue `/better-interface` comme les autres surfaces du PRD |

## Sources

- Obsidian, *Basic formatting syntax* et *Embed files*, sources Markdown de l'aide officielle : <https://github.com/obsidianmd/obsidian-help> (`en/Editing and formatting/Basic formatting syntax.md:216-225`, `en/Linking notes and files/Embed files.md:46-62`), publiées sur <https://help.obsidian.md/syntax> et <https://help.obsidian.md/embeds>.
- Obsidian, *Attachments* (emplacements des pièces jointes) : <https://obsidian.md/help/attachments>.
- Notion, *Images, files & media* (poignées gauche et droite, alignement, légende, recadrage, plein écran, remplacement, limite de 5 Mo en offre gratuite) : <https://www.notion.com/help/images-files-and-media>.
- CommonMark 0.31.2 (images, HTML brut) : <https://spec.commonmark.org/0.31.2/>.
- Pandoc, extension `link_attributes` : <https://pandoc.org/MANUAL.html>.
- Typora, *Resize Image* : <https://support.typora.io/Resize-Image/>.
- GPUI, rev `37faef81f74fc1bb08be9939c4cffc8ede6ced2f` : fichiers et lignes cités au §3.
- writer-computer : `SPECs/obsidian-image-embed-spec.md:19` ; Paneflow : `src-app/src/terminal/input.rs:1104-1165`, `src-app/src/terminal/clipboard_image.rs`, `src-app/src/app/sidebar/drop.rs:112-131`, `src-app/src/app/diff_dock/code/mod.rs:22-36`.
