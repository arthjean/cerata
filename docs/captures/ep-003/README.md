# Captures d'EP-003 (Windows)

Porte UI d'EP-003 sur la machine de référence Windows (`docs/benchmarks.md`), le 2026-11-01 : `cerata.exe` (build debug) ouvert sur des notes du type du corpus et piloté par de vraies entrées de l'OS (souris et clavier par `SendInput`, dispositions AZERTY et MS-IME japonais, presse-papiers Windows), chaque étape capturée dans la zone cliente de la fenêtre.

| Fichier | Story | Ce qu'elle montre |
|---|---|---|
| `us-016-rendering.png` | US-016 | Titres à leur propre taille, colonne de 734 px centrée, syntaxe masquée, puces et cases |
| `us-017-ime-ms-ime-japanese.png` | US-017 | Touche morte AZERTY (`ê`) ; composition MS-IME soulignée au caret, fenêtre de candidats juste dessous ; Échap efface la pré-édition et l'annulation suivante retire `ê` (la composition n'est jamais entrée dans l'historique) ; `日本語` validé puis annulé en un pas |
| `us-017-hold-down.png` | US-017 | Flèche bas maintenue dans une note longue : le caret reste dans les marges de sécurité |
| `us-018-mouse.png` | US-018 | Clic à travers la syntaxe masquée, zone du préfixe de liste, double et triple clic, glisser depuis un préfixe, clic sur une image (décoration sélectionnée, ancrée à sa fin), case cochée, glisser relâché hors de la fenêtre |
| `us-019-code-scroll.png` | US-019 | Molette horizontale sur un bloc de code, molette verticale qui fait défiler la page, barre du frontmatter glissée, Fin et Origine qui révèlent le caret au plus petit déplacement. Sa dernière vue précède la correction du bloc rétréci |
| `us-019-narrowed-block.png` | US-019, US-018 | Après correction : la sélection d'une ligne de code défilée reste dans le bloc ; supprimer la longue ligne ramène le décalage à 0 et retire la barre |
| `us-020-remote-http-data.png` | US-020 | Image distante en `https:` affichée, la même en `http:` jamais chargée (source atténuée), SVG en URI `data:` rendu |
| `us-021-find.png` | US-021 | Ctrl+F pré-rempli par la sélection, `1/3` insensible à la casse, Entrée, Maj+Entrée, Ctrl+G, `0/0` avec le champ signalé, Entrée sans effet, Échap qui rend le focus |
| `us-021-replace-and-marks.png` | US-021 | Marque de la barre cliquée (résultat à 140 px du bord), Remplacer puis résultat suivant, Tout remplacer, un seul Ctrl+Z qui restaure tout |
| `us-022-context-menus.png` | US-022 | Menu du corps dessiné dans l'app, Coller désactivé presse-papiers vide, Ouvrir et Copier le lien sur un lien |
| `us-020-us-022-clipboard-and-paste.png` | US-020, US-022 | Sous-menu Format avec les raccourcis du registre, copier, couper et coller en texte brut, frontmatter d'un document collé envoyé en tête, image collée enregistrée dans `{stem}-assets/` et affichée, image de 8,6 Mo refusée avec la notice |

Microsoft Pinyin sous Windows reste une vérification humaine (US-045) ; Linux et macOS relèvent de US-045 et US-046.
