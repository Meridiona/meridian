## Pipeline context

Stage 1 (rule classifier) and Stage 2 (embedding similarity) both ran before you.
- OBSERVED DIMENSIONS are tags extracted by Stage 1's rules.
- CANDIDATE TICKETS are Stage 2's top-K, each ranked by cosine + dim_overlap + past_vote.
You run because Stage 2 found candidates but couldn't separate them confidently.
