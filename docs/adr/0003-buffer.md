# ADR 0003 : buffer de texte, `String` avec index de lignes

> Statut : **décidé** le 2026-11-01. Les crates `text` et `rope` de Zed sont écartées ; le buffer de `cerata_markdown` est une `String` UTF-8 avec un index des débuts de ligne.
> Story : US-009. Plan : `PLAN.md` §3.4 (point 1, buffer), §3.7 (crates sans GPUI). ADR liées : 0001 (moteur d'édition), 0002 (parser).

## Contexte

US-009 demande un état d'éditeur (texte, sélections multiples, transactions, historique) testable sans fenêtre, et une décision écrite sur le buffer. Le plan liait ce choix à la réponse de Joel (Writer) sur la publication App Store, au motif que réutiliser les crates de Zed pouvait poser une question de licence. Deux candidats : les crates `text` et `rope` de Zed (rope en `sum_tree`, ancres, CRDT), ou un buffer propre.

## Faits

- `text` et `rope` de Zed dépendent de `gpui_util` (au rev GPUI de l'ADR 0001). Une contrainte dure du PRD interdit toute dépendance `gpui*` dans `cerata_core` et `cerata_markdown`, et `scripts/check-core-deps.sh` la vérifie en CI sur tous les types d'arêtes. Ces crates sont donc exclues par l'architecture, avant toute question de licence : **la décision ne dépend pas de la réponse de Joel**.
- Le parser retenu (ADR 0002, pulldown-cmark) lit un `&str` contigu. Une rope obligerait à recopier le document à chaque reparse, ce qui annule son avantage.
- L'ADR 0001 a mesuré l'insertion dans une `String` de 1 Mo avec index de lignes sous 1 ms ; le budget frappe vers frame est de 16 ms.
- Le reparse fenêtré de US-010 tient 0,29 ms au p99 sur la liste de 3 000 items et 2,0 ms au p99 sur 1 Mo (`cargo run --release -p cerata_markdown --example tree_bench`), buffer compris dans le même processus.

## Décision

`cerata_markdown::Text` : une `String` et le vecteur des débuts de ligne (sauts `\n`, `\r\n` et `\r` seul, comme CodeMirror). Une édition remplace la plage dans la `String` puis recalcule l'index seulement à partir de la ligne touchée, en décalant la suite. Les positions sont des octets UTF-8 ; la conversion en UTF-16 pour l'IME reste au crate `cerata` (EP-003).

Les ancres (`Anchor`, `ScrollAnchor`) et les sélections sont reportées par `ChangeSet::map_pos` à chaque transaction, sans identifiants persistants : c'est le modèle de CodeMirror 6, celui que Writer exerce dans ses tests.

## Conséquences

- Aucune dépendance de Zed dans les crates sans GPUI ; aucune question de licence pour le buffer.
- Une insertion coûte O(n) en octets déplacés (memmove), négligeable à 1 Mo ; au-delà de quelques dizaines de Mo, une rope redeviendrait utile. Writer n'ouvre pas de tels fichiers, et le PRD ne fixe pas de budget au-delà de 1 Mo.
- Pas de CRDT ni d'édition collaborative : hors périmètre du PRD.

## Non prouvé

- Mesures sous Windows seulement ; macOS et Linux suivent US-045 et US-046.
