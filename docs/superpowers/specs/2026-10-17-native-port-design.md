# Portage natif de Herdr Queue

## Objectif approuvé

Remplacer Python et Textual par un exécutable Rust avec Ratatui. L'utilisateur ne doit
pas installer Python, uv ou Rust lorsqu'il utilise un binaire distribué. Conserver les
fonctionnalités existantes et viser moins de 20 Mio RSS au repos, à vérifier par mesure.
La démo Python de référence atteint 49,64 Mio de pic RSS dans un pseudo-terminal 120×40,
avec deux messages et sept secondes d'ouverture sur macOS.

## Compatibilité

- Garder l'identifiant du plugin `herdr-drafts`, l'action `open`, le panneau `drafts`,
  les variables d'environnement et les commandes `connect`, `open`, `tui`, `demo`, `doctor`.
- Lire et modifier la base SQLite existante sans migration destructive. Conserver les
  clés SHA-256 calculées à partir du JSON Python, y compris espaces et échappements Unicode.
- Garder les brouillons, le texte en cours, l'ordre, l'historique, l'adoption initiale
  d'une session native et les verrouillages exclusifs par terminal.
- Garder Entrée pour ajouter, Maj+Entrée/Ctrl+J pour une nouvelle ligne, les raccourcis
  Cmd+U/Y/L/P/O/R, les équivalents Ctrl/F et la souris.
- Respecter le fond et la palette ANSI du terminal et les terminaux de petite taille.

## Architecture

Un seul processus Rust, sans serveur ni service résident. Ratatui/Crossterm dessinent
l'interface après un changement et attendent des événements entre les mises à jour.
Les accès réseau passent par de courts workers ; l'interface ne se bloque pas pendant
une réponse lente. SQLite conserve les transactions, le délai de verrouillage et WAL.
Les états incertains ne sont jamais automatiquement renvoyés.

Les modules concrets sont `models`, `store`, `herdr`, `discovery`, `service`, `ui` et
`entry`. Le stockage décide seulement des transitions atomiques. Le service valide le
texte, l'appartenance et la conversation ; le transport borne les réponses et qualifie
l'incertitude. L'interface gère les événements et les confirmations.

## Invariants d'envoi

Revalider socket, terminal, panneau, espace, agent, dossier et session avant tout envoi.
Refuser un agent bloqué ou inconnu. Limiter un message à 65 536 octets UTF-8 et refuser
les contrôles de terminal hors tabulation et nouvelle ligne. Une tentative revendique
atomiquement son brouillon. Une réponse interrompue après écriture devient incertaine ;
un échec de connexion avant écriture reste réessayable. Aucun vidage automatique de file.
Le raccourci suivant conserve le texte du composeur et respecte l'ordre de la file.

## Distribution et validation

Le lanceur utilise un binaire local ou téléchargé et vérifié par SHA-256. Les sources
restent compilables avec Cargo. Une CI construit et teste macOS/Linux et produit les
archives natives ; les utilisateurs d'un binaire n'installent pas de compilateur.
Tester la compatibilité SQLite, le transport fragmenté, les refus, l'incertitude, les
raccourcis, le collage multiligne, Unicode, la souris et le redimensionnement. Comparer
le pic RSS avec la démo Python dans le même terminal, séparément du poids sur disque.
