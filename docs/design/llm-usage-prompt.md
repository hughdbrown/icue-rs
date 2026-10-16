# Purpose
Make a widget for the Corsair Xeneon Edge using the Rust API in this directory tree that does this:
- shows all the user's LLM subscriptions
- shows how much quota is left until it runs out
- shows when the quota refreshes

# Goal
Allow a technical user to maximize usage of LLM subscription by knowing when quota for a subscription becomes exhausted.

# Supported LLMs
- anthropic.com / Claude
-- commonly done as a 5 hour block of time to exhaust quota measured in percentages
- OpenAI / codex
- google / gemini
- kimi.com / kimi
- ollama.com / various (including kimi)
-- commonly done as a single block of time (one month) to exhaust dollars of quota 

# User benefit
A user who has LLM subscriptions benefits most from exhausting quota in every timeframe, on the assumption that no LLM quota should be wasted or expired without being used. Accordingly, we want to have the user come as close as possible to exhaustion of quota at the time limit as possible. If the quota is used prematurely, then ideally the user has quota in a different subscrtiption available to continue.

# Stack
- Rust 1.99 or higher
- 

# Deliverables
- Rust code that implements the widget logic
- compiled code for widget logic
- HTML / JS / CSS that implements the widget's UI
- example format of config file for LLMs to cover with credentials (file saved as 600 in production)
- git commits of code saved
- justfile for testing, building, and deploying the binaries

# Required actions
- research how the information can be obtained for each LLM
