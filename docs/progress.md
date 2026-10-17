# État du développement

- Cadrage : le développement autonome demandé autorise la mise en œuvre du plan présenté.
- Isolation : dépôt neuf `herdr-drafts`, branche `feat/initial-plugin`, aucun worktree nécessaire.
- Vault : aucun projet correspondant ; aucune création ni modification dans le vault.
- Documentation : Context7 Herdr et Textual consultés ; schéma Herdr 0.9.3 exporté localement.
- Préflight : le stockage sera utilisé par le service et l'interface ; les types communs
  résident dans `models.py`. Pas d'interface abstraite ni de serveur intermédiaire.
- Stockage : 10 tests validés (reprise, édition, ordre, isolation, verrou, permissions).
- Transport et service : 26 tests supplémentaires validés ; comparaison au code Herdr v0.9.3
  et à son schéma local. La confirmation de soumission est `agent_prompted`.
- Contrainte d'environnement : l'ouverture de sockets nommés est interdite. Le connecteur
  testable reçoit de vrais flux Unix `socketpair`, sans port réseau ni session utilisateur.
  L'intégration dans une vraie session sera exécutée ensemble hors de cette restriction.
- Dépendances : téléchargement des wheels officiels PyPI avec curl puis installation locale,
  car le résolveur uv n'a pas d'accès DNS dans son sandbox. Aucun outil global installé.
- Interface : saisie, souris, confirmation, historique et panneau 36×24 vérifiés avec Pilot.
- Packaging : manifeste, lanceur, démo et diagnostic ; wheel et archive source construits.
  Lockfile portable produit à partir des métadonnées officielles PyPI (URLs et SHA-256),
  puis vérifié par `uv sync --frozen --dry-run`.
- Revue indépendante : trois anomalies reproduites sur la première session native tardive,
  sa restauration et la réouverture après remplacement de conversation.
- Corrections : relation durable de l'identité initiale vers la première session, résolution
  avant récupération et création de l'éditeur, récupération des textes préparés pendant la
  fermeture, refus explicite de refocaliser un panneau lié à une autre conversation.
  Les trois régressions étaient rouges, puis les tests ciblés sont passés.
- Décision : une conversation remplacée conserve son ancienne file. Il faut fermer son
  panneau avec Ctrl+Q avant d'ouvrir celui de la nouvelle conversation ; aucune migration
  automatique des textes d'une conversation connue vers une autre.


- Final : 54 tests réussis, dont régression Pilot de restauration du compositeur.
  Ruff et mypy sans erreur ; démo lancée dans un PTY et fermée avec Ctrl+Q.
- Les trois anomalies de la revue ont été corrigées et couvertes. Aucun problème mineur différé.
- Livraison locale conservée pour le test commun ; aucun dépôt distant ni publication créé.

## Terminal autonome — 17 octobre 2026

- Le lancement sans argument ouvre un sélecteur d'agents puis les brouillons dans le même
  terminal. Les entrées `open` et `tui` conservent le panneau intégré.
- Découverte des sessions locales avec le registre CLI Herdr ; priorité au socket hérité
  ou explicite, et sélection d'une session possible. Les snapshots enrichissent les agents
  avec les noms de projet, d'onglet et de session. Une session inaccessible laisse les
  autres agents disponibles et affiche son erreur.
- Recherche, choix explicite au clavier ou à la souris, actualisation toutes les cinq
  secondes. Le bouton Agents et Ctrl+G sauvegardent le compositeur avant le retour au choix.
- Le mode autonome ne crée, ne déplace et ne ferme aucun panneau Herdr. Le stockage et
  les verrous partagent le répertoire natif `herdr/plugins/herdr-drafts`.
- Validation : 74 tests réussis, Ruff et mypy sans erreur, archives construites avec les
  nouveaux styles inclus. Lanceur sans argument et fermeture Ctrl+Q vérifiés dans un PTY.
  Revue indépendante sans problème bloquant ; intégration réelle à tester hors du sandbox.

## Couleurs du terminal — 17 octobre 2026

- Diagnostic : le thème `textual-dark` et les couleurs RVB des deux feuilles de style
  imposaient un fond sombre malgré la palette claire du terminal.
- Le sélecteur et les brouillons utilisent désormais `textual-ansi`, initialisé avant
  le rendu, et les variables du thème. Les composants conservent les couleurs natives
  au focus, dans le compositeur et dans les confirmations.
- Textual 6.6 perd la valeur ANSI par défaut lors de certaines conversions Rich :
  elle devient du noir RVB. Un filtre de rendu rétablit la valeur native dans cette
  interface ANSI ; le noir explicitement choisi dans la palette ANSI reste intact.
  Réévaluer ce contournement lors d'une mise à jour de Textual.
- Les effets de teinte et de transparence des boutons sont désactivés : ils supposent
  une couleur RVB connue et échouent avec le fond natif. Le focus utilise l'inversion.
- Validation : 76 tests réussis, Ruff et mypy sans erreur. Démo et sélecteur lancés dans
  un PTY avec une configuration Ghostty : fonds natifs, aucun fond RVB émis, sortie Ctrl+Q.
