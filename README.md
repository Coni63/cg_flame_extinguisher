# Extinction des Feux (Bolgrot) - Game Documentation

## 🎯 Objectif

Éteignez toutes les flammes sur la carte tout en conservant le maximum de points de vie (**PV**). 

* **Score final :** Nombre de PV restants à la victoire.
* **Inspiration :** Combat tactique contre Bolgrot dans la quête *"Extinction des feux"* de Dofus (Ankama Games).

---

## 🗺️ Carte et Ressources

### Carte
* **Dimensions :** Grille fixe de **35 x 34** (Largeur $Width = 35$, Hauteur $Height = 34$).
* **Coordonnées :** $(0, 0)$ en haut à gauche. $X$ augmente vers la droite, $Y$ augmente vers le bas.
* **Bordures et obstacles :** Les bordures sont toujours constituées de murs (`#`).

### Ressources de départ
* **PV (Points de Vie) :** Commence à **40**.
* **PA (Points d'Action) :** Commence à **10** (réinitialisés à chaque `PASS`).

---

## 🔄 Tours et Cycles

* **Tour :** Une itération de boucle I/O (réception d'entrées, envoi d'actions).
* **Cycle :** Une séquence d'actions qui se termine par un `PASS`. Un cycle peut s'étendre sur un ou plusieurs tours.

### Apparition des Glyphes et Flammes
1. **6 premiers cycles :** De nouveaux glyphes (marques violets) apparaissent au début de chaque cycle.
2. Si aucun glyphe n'apparaît au début d'un cycle, le joueur perd **1 PV**.
3. À la fin d'un cycle (`PASS`), tous les glyphes se transforment en flammes (sauf si la case est occupée par le joueur ou une autre flamme). Le glyphe disparaît dans tous les cas.

---

## 🕹️ Actions Disponibles

Vous pouvez envoyer une ou plusieurs actions par tour, séparées par des points-virgules `;` (ex: `MOVE N;JUMP E;PASS`).

| Action | Direction | Coût PA | Coût PV | Description |
| :--- | :--- | :---: | :---: | :--- |
| `MOVE dir` | `N`, `S`, `E`, `W` | 1 | 1 | Déplace de 1 case. La case cible ne doit pas être un mur. |
| `JUMP dir` | `N`, `S`, `E`, `W` | 2 | 1 | Sauts de 2 cases. Max **2 sauts par cycle**. La case cible ne peut pas être un mur (la case intermédiaire peut l'être). |
| `IMMO dir` | `NE`, `NW`, `SE`, `SW` | 1 | 5 | Reste sur place et attire toutes les flammes vers la case diagonale ciblée. |
| `PASS` | - | 0 | 0 | **Termine le cycle**. Restaure **10 PA** et réinitialise le compteur de sauts à **0**. |

---

## ⚙️ Mécaniques et Effets des Actions

### 1. Effets d'un Déplacement (`MOVE` ou `JUMP`)

Lors d'un déplacement, les événements surviennent dans cet ordre précis :

1. **Déplacement & Perte de PV :** Le personnage se déplace et perd 1 PV. *(Attention : si le personnage a 1 PV avant de se déplacer, il meurt immédiatement)*.
2. **Extinction de Flamme :** Si la case d'arrivée contient une flamme, elle est éteinte et le **1 PV perdu est remboursé**.
3. **Poussée des Flammes Adjacentes :** Chaque flamme adjacente (8 voisins autour de la case d'arrivée) est repoussée d'1 case dans la même direction relative au joueur.
   * *Bloqueurs :* Une flamme est bloquée si sa destination est un mur, le personnage ou une autre flamme.
   * *Danger :* **Si une flamme est bloquée lors de la poussée, le personnage meurt instantanément.**
4. **Attraction des Flammes :** Toutes les flammes sont attirées d'une case vers la case de destination du joueur :
   * Traitées par ordre croissant de **distance de Manhattan** à la destination (départagées dans le sens horaire depuis le Nord en cas d'égalité).
   * Suivent la trajectoire la plus directe (cardinale si alignées, diagonale si exacte, sinon axe dominant).
   * *Danger :* **Si une flamme est attirée sur la case du personnage, celui-ci meurt instantanément.**

### 2. Effets de `IMMO`

1. Le personnage reste sur sa case et perd **5 PV**.
2. **Attraction :** Toutes les flammes sont attirées d'1 case vers la case diagonale ciblée.
3. *Sécurité :* Pendant `IMMO`, la case du personnage agit comme un mur. Aucune flamme ne peut s'y déplacer et cela ne provoque **pas de mort instantanée**.

---

## 🏆 Conditions de Fin de Partie

* **Victoire :** Toutes les flammes sont éteintes ET il ne reste aucun glyphe.
* **Défaite :** 
  * Mort du personnage (PV $\le 0$ ou écrasement par/sur une flamme).
  * Action illégale, PA insuffisants, direction invalide ou dépassement du temps de réponse (Timeout).

---

## 📥 Protocole de Jeu

### Entrées d'Initialisation (Exécutées une seule fois)
* **Ligne 1 :** `height width` (Toujours `34 35`).
* **`height` Lignes suivantes :** Chaîne de caractères de longueur `width` représentant la carte (`#` = mur, `.` = libre, `S` = point de départ).

### Entrées pour chaque tour
* **Ligne 1 :** `charX charY` (Position actuelle).
* **Ligne 2 :** `hp ap jumpsLeft` (PV restants, PA restants, sauts restants dans le cycle).
* **Ligne 3 :** `flameCount` (Nombre de flammes).
* **`flameCount` Lignes suivantes :** `x y` (Positions des flammes).
* **Ligne suivante :** `glyphCount` (Nombre de glyphes).
* **`glyphCount` Lignes suivantes :** `x y` (Positions des glyphes).
* **Ligne suivante :** `validActionCount` (Nombre d'actions valides reçues).
* **`validActionCount` Lignes suivantes :** `validAction` (Ex: `PASS`, `MOVE N`, `JUMP E`, etc.).

### Sortie pour chaque tour
* Une ligne contenant une ou plusieurs actions séparées par `;` (Exemple : `MOVE N;MOVE E;PASS`).
* Toutes les actions situées après un `PASS` dans la même chaîne seront ignorées.

---

## ⏱️ Contraintes

* $0 \le \text{charX}, \text{flameX}, \text{glyphX} < 35$
* $0 \le \text{charY}, \text{flameY}, \text{glyphY} < 34$
* $1 \le \text{hp} \le 40$
* $0 \le \text{ap} \le 10$
* $0 \le \text{jumpsLeft} \le 2$
* $0 \le \text{flameCount} \le 36$
* $0 \le \text{glyphCount} \le 6$
* **Temps de réponse maximal :** $< 100 \text{ ms}$ par tour.