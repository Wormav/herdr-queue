# Test commun depuis un autre terminal

Ce parcours utilise une conversation de test choisie explicitement. Les tests automatisés
n'ont pas validé le comportement de vos versions de CLI pendant leur exécution.

1. Lancer `./run.sh demo`. Ajouter deux messages, les réordonner, modifier le second,
   envoyer uniquement celui-ci, puis le retrouver dans l'historique. Fermer avec Ctrl+Q.
2. Lancer un agent de test dans Herdr. Dans un autre terminal, exécuter
   `/Users/jeremy/Documents/Github/herdr-drafts/run.sh`.
   La `.venv` de ce checkout est déjà préparée ; ailleurs, le lanceur prépare ses dépendances.
3. Vérifier le projet, le nom et l'état dans la liste. Rechercher le projet, puis choisir
   l'agent avec Entrée ou un clic. Vérifier le destinataire au-dessus des brouillons et
   leur affichage dans le terminal de lancement.
4. Pendant que l'agent travaille, préparer trois messages. Vérifier qu'ils restent dans
   le panneau. Utiliser Entrée pour ajouter, et Maj+Entrée pour insérer une nouvelle ligne
   dans un message. Envoyer le deuxième seulement et observer le texte reçu, dont les nouvelles lignes.
5. Garder un texte inachevé, cliquer sur Agents ou utiliser Ctrl+G, puis choisir un autre
   agent. Revenir au premier : les deux messages restants et le texte inachevé doivent
   être conservés. Fermer avec Ctrl+Q, relancer et vérifier de nouveau la restauration.
6. Déclencher une demande interactive dans la conversation de test. Un envoi doit être
   refusé et conservé. Résoudre la demande dans le terminal de l'agent.
7. Refaire les étapes 3–5 avec les autres CLI : Codex, Claude Code et Antigravity.
8. Dans la liste, vérifier Ctrl+R et la mise à jour automatique après le lancement ou
   l'arrêt d'un agent. Une recherche sans résultat doit pouvoir être effacée. Tester
   `./run.sh --session default` si plusieurs sessions Herdr sont présentes.
9. Pour tester aussi le panneau intégré, lier le plugin avec
   `herdr plugin link /Users/jeremy/Documents/Github/herdr-drafts`, sélectionner un agent
   dans Herdr puis invoquer `herdr-drafts.open` dans la palette d'actions. Vérifier le
   placement à droite. Fermer cette interface avant de choisir le même agent depuis
   le terminal autonome : les brouillons doivent être partagés.

La démo simule les transmissions ; seul le parcours en session réelle confirme l'intégration
du terminal, de la souris et de chaque CLI. Aucun changement de raccourci n'est nécessaire.
