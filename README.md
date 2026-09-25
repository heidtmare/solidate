# Solidate: Knowledge for humans!... and also AI!  

Solidate is clean, efficient, wiki-style knowledge base.  
The fundamental design goal is to create a single place for project design documentation to exist than can be easily consumed and manipulated by AI tools, but is also very much readable and modifiable by normal human beings.

Markdown(.md) is the format of choice, but documentation can be varied by nature and we must consider such variance as part of the game.
Documentation by AI for AI can constitute differently, with different goals and alignment. The trick is automatically keeping everything in sync so all parties are dancing to the same tune, even if they read sheet music with different scales.

## Goals & Considerations 
- Clean ui's with little in the way. fast server-side rendering with none of that js framework nonsense. 
- Smart hashing techniques for ensuring up-to-date-ness.
- Complete discoverability and organization for all parties involved.
- API's supporting access and updates by external tools.  
- Multi-project divides, but with shared inheritance to support cross-project rules, branding, or other considerations.
- Multi-tenant archecture. shards or something else to allow disperate groups to coexist on a resource pool.

## Tech Stack
- Rust as the primary language
- Topcoat as the web framework
- Comrak for Markdown parsing
- SQLx and PostgreSQL for the data layer
