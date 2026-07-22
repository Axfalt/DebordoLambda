#!/usr/bin/env python3
"""
Script pour enregistrer les commandes slash Discord "debordo" et "register-key".
Exécuter une seule fois après avoir créé l'application Discord (ou à chaque modification de commande).

Usage:
    python register_command.py

Variables d'environnement requises:
    DISCORD_BOT_TOKEN: Token du bot Discord
    DISCORD_APPLICATION_ID: ID de l'application Discord
"""

import os
import requests

BOT_TOKEN = os.environ.get("DISCORD_BOT_TOKEN", "")
APPLICATION_ID = os.environ.get("DISCORD_APPLICATION_ID", "")

if not BOT_TOKEN or not APPLICATION_ID:
    print("❌ Veuillez définir DISCORD_BOT_TOKEN et DISCORD_APPLICATION_ID")
    exit(1)

url = f"https://discord.com/api/v10/applications/{APPLICATION_ID}/commands"

commands = [
    {
        "name": "debordo",
        "description": "Calcule la probabilité de débordement d'une ville",
        "options": [
            {
                "name": "defense",
                "description": "Valeur de défense de la ville",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "tdg_min",
                "description": "Estimation minimale de la TDG",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "tdg_max",
                "description": "Estimation maximale de la TDG",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "min_def",
                "description": "Défense minimale en maison",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "nb_drapo",
                "description": "Nombre de drapeaux (défaut: 0, fonctionnalité legacy)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "day",
                "description": "Jour de la simulation (défaut: 1)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "iterations",
                "description": "Nombre d'itérations (défaut: 10000)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "reactor",
                "description": "Le réacteur est-il construit? (défaut: false)",
                "type": 5,  # BOOLEAN
                "required": False
            },
            {
                "name": "nb_hab",
                "description": "Nombre de personnes en ville (défaut: 40)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "interactive",
                "description": "Ouvrir un formulaire (modal) pré-rempli pour ajuster la configuration (défaut: false)",
                "type": 5,  # BOOLEAN
                "required": False
            }
        ]
    },
    {
        "name": "debordo-complete",
        "description": "Simulation détaillée avec risque de mort par citoyen",
        "options": [
            {
                "name": "defense",
                "description": "Valeur de défense de la ville",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "tdg_min",
                "description": "Estimation minimale de la TDG",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "tdg_max",
                "description": "Estimation maximale de la TDG",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "min_def",
                "description": "Défense minimale en maison",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "nb_drapo",
                "description": "Nombre de drapeaux (défaut: 0, fonctionnalité legacy)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "day",
                "description": "Jour de la simulation (défaut: 1)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "iterations",
                "description": "Nombre d'itérations (défaut: 10000)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "reactor",
                "description": "Le réacteur est-il construit? (défaut: false)",
                "type": 5,  # BOOLEAN
                "required": False
            },
            {
                "name": "nb_hab",
                "description": "Nombre de personnes en ville (défaut: 40)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "defenses",
                "description": "Défenses nominatives (ex: 'Axfalt:15, Bob:8')",
                "type": 3,  # STRING
                "required": False
            },
            {
                "name": "home_bonus",
                "description": "Bonus de défense fixe appliqué à chaque maison (ex: 4)",
                "type": 4,  # INTEGER
                "required": False
            },
            {
                "name": "interactive",
                "description": "Ouvrir un formulaire (modal) pré-rempli pour ajuster la configuration",
                "type": 5,  # BOOLEAN
                "required": False
            }
        ]
    },
    {
        "name": "register-key",
        "description": "Enregistre votre ExternalID MyHordes de manière sécurisée",
        "options": []
    }
]

headers = {
    "Authorization": f"Bot {BOT_TOKEN}",
    "Content-Type": "application/json"
}

# Utiliser PUT pour écraser/enregistrer toutes les commandes globales à la fois
response = requests.put(url, json=commands, headers=headers)

if response.status_code in (200, 201):
    print("✅ Commandes slash enregistrées avec succès!")
    print(f"   Réponse: {response.json()}")
else:
    print(f"❌ Erreur lors de l'enregistrement: {response.status_code}")
    print(f"   Réponse: {response.text}")
