#!/usr/bin/env python3
"""
Script pour enregistrer la commande slash Discord "debordo".
Exécuter une seule fois après avoir créé l'application Discord.

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

command = {
    "name": "debordo",
    "description": "Calcule la probabilité de débordement d'une ville",
    "options": [
        {
            "name": "defense",
            "description": "Valeur de défense de la ville",
            "type": 4,  # INTEGER
            "required": True
        },
        {
            "name": "tdg_min",
            "description": "Estimation minimale de la TDG",
            "type": 4,  # INTEGER
            "required": True
        },
        {
            "name": "tdg_max",
            "description": "Estimation maximale de la TDG",
            "type": 4,  # INTEGER
            "required": True
        },
        {
            "name": "min_def",
            "description": "Défense minimale en maison",
            "type": 4,  # INTEGER
            "required": True
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
            "name": "nb_habitant",
            "description": "Nombre de personnes en ville (défaut: 40)",
            "type": 4,  # INTEGER
            "required": False
        }
    ]
}

headers = {
    "Authorization": f"Bot {BOT_TOKEN}",
    "Content-Type": "application/json"
}

response = requests.post(url, json=command, headers=headers)

if response.status_code in (200, 201):
    print("✅ Commande /debordo enregistrée avec succès!")
    print(f"   Réponse: {response.json()}")
else:
    print(f"❌ Erreur lors de l'enregistrement: {response.status_code}")
    print(f"   Réponse: {response.text}")

