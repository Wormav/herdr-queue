# Sélection d'un agent depuis un autre terminal

Demande : lancer Herdr Drafts dans un terminal distinct, retrouver les agents Herdr actifs,
choisir un agent puis afficher ses brouillons dans le terminal de lancement.

Le lancement sans argument devient `connect`. Les commandes `open` et `tui` du manifeste
conservent le panneau intégré. Le sélecteur utilise les snapshots Herdr pour afficher les
projets, les noms et les états, avec recherche et actualisation. Les sessions locales sont
découvertes par `herdr session list --json` ; un socket explicite ou le contexte hérité est
respecté. La sélection est toujours explicite, y compris lorsqu'un seul agent existe.

Le sélecteur et les brouillons s'exécutent successivement dans le même terminal. Aucun appel
de création, de focus ou de fermeture d'un panneau Herdr n'est effectué en mode autonome.
Un bouton Agents et Ctrl+G permettent de changer de cible après sauvegarde du compositeur.
Le stockage et les verrous sont partagés avec le plugin intégré.

1. [x] Tests rouges puis découverte et snapshots enrichis : labels, sessions, erreurs partielles.
2. [x] Tests Pilot rouges puis sélecteur : recherche, clic, clavier, vide, erreurs, disparition.
3. [x] Tests rouges puis lancement autonome et retour au sélecteur sans perte des brouillons.
4. [x] Documentation, contrôles Python, build et revue du diff.

Validation : 74 tests réussis, Ruff et mypy sans erreur, wheel et archive source construites.
Le lanceur sans argument ouvre le sélecteur dans un PTY et Ctrl+Q restitue le terminal.
La revue indépendante ne relève aucun problème bloquant. La connexion aux agents réels
reste à vérifier dans le terminal utilisateur, hors des restrictions de socket du sandbox.

Le test contre les agents réels est en lecture seule pendant le développement. Aucune soumission
de prompt de test aux conversations de l'utilisateur.
