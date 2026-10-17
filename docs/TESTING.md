# Test commun dans Herdr

Ce parcours utilise une conversation de test choisie explicitement. Les tests automatisés
n'ont pas validé le comportement de vos versions de CLI pendant leur exécution.

1. Lancer `./run.sh demo`. Ajouter deux messages, les réordonner, modifier le second,
   envoyer uniquement celui-ci, puis le retrouver dans l'historique. Fermer avec Ctrl+Q.
2. Lier le plugin avec `herdr plugin link /Users/jeremy/Documents/Github/herdr-drafts`.
   La `.venv` de ce checkout est déjà préparée ; ailleurs, exécuter d'abord `uv sync --frozen --no-dev`.
3. Sélectionner une conversation de test dans Herdr, puis invoquer `herdr-drafts.open`
   dans la palette d'actions. Vérifier le destinataire affiché et le placement à droite.
4. Pendant que l'agent travaille, préparer trois messages. Vérifier qu'ils restent dans
   le panneau. Envoyer le deuxième seulement et observer le texte reçu, dont les nouvelles lignes.
5. Fermer avec un texte inachevé dans le compositeur, puis rouvrir. Vérifier la présence
   des deux messages restants et du texte inachevé.
6. Déclencher une demande interactive dans la conversation de test. Un envoi doit être
   refusé et conservé. Résoudre la demande dans le terminal de l'agent.
7. Refaire les étapes 3–5 avec les autres CLI : Codex, Claude Code et Antigravity.

La démo simule les transmissions ; seul le parcours en session réelle confirme l'intégration
du terminal, de la souris et de chaque CLI. Aucun changement de raccourci n'est nécessaire.
