# Video Classification Rules for Health, Dieting, Weight Loss Content

**Last Updated**: 2026-03-07  
**Purpose**: Rules for `ingest_video_transcription` tool to classify clips. Loaded dynamically. Refine keywords/phrases over time. Uses keyword matching + embedding similarity for topics, sentiment, pros/cons, risks.

## Topics (Keywords/Phrases for Matching)
- **Weight Loss**: weight loss, lose weight, fat loss, calorie deficit, metabolism, BMI, body composition
- **Dieting/Nutrition**: diet, keto, intermittent fasting, low carb, high protein, macros, meal prep, nutrition, supplements, vitamins
- **Fitness/Exercise**: workout, exercise, cardio, strength training, HIIT, gym, running, muscle building, toning
- **Health**: healthy lifestyle, wellness, mental health, sleep, hydration, gut health, hormones
- **Before/After**: transformation, before after, progress, results, journey

## Sentiment Indicators
- **Positive**: success, effective, sustainable, energy boost, confident, motivated, results, improved, better health
- **Negative**: struggle, difficult, failure, yo-yo, restrictive, frustrated, unhealthy, dangerous, side effects
- **Neutral**: information, explanation, science, study, research, facts, method, approach

## Pros (Positive Aspects to Flag)
- Evidence-based, science-backed, sustainable habits, balanced approach, professional advice, realistic expectations, combined diet+exercise, mental health focus, long-term results, disclaimers present

## Cons/Risks (Monetization & Content Risks - YouTube Policies)
**High Risk for Demonetization/Limited Ads (from YouTube Advertiser-Friendly + Monetization Policies)**:
- **Misleading Claims**: "Lose weight without diet/exercise", "guaranteed results", "miracle cure", "10kg in 1 week", "no effort needed", unproven supplements as "fat burners"
- **Dangerous Advice**: Extreme calorie restriction, fasting without medical supervision, promoting eating disorders (anorexia triggers, restrictive diets for minors), unsafe challenges
- **Before/After Issues**: Dramatic transformations without disclaimers ("results not typical", "individual results vary", "consult doctor"), implying guarantees, edited/misrepresented photos
- **Medical/Health Claims**: Treating as medical advice without qualifications ("cures diabetes", "fixes hormones"), pseudoscience, conspiracy about "Big Pharma"/diets
- **Other**: Harmful acts (extreme workouts risking injury without warnings), shocking content (graphic body shaming), unreliable content (fad diets without evidence), controversial (eating disorders)

**YouTube Specific (2025-2026 Policies)**:
- Follow Community Guidelines, Advertiser-Friendly: No harmful/unreliable health content, no promotion of dangerous acts.
- Sensitive: Eating disorders, self-harm related to body image.
- Inauthentic: Repetitive "transformation" spam without value.
- Always include disclaimers for health/weight claims.

**Risk Scoring**: Flag if >2 risk keywords; add `risk_reason` in frontmatter.

## Best Practices for Weight Loss/Fitness Channels (for Positive Classification)
- Science-based references/studies
- Professional disclaimers ("not medical advice, consult doctor")
- Sustainable, balanced approaches (not extreme)
- Focus on habits, mindset, long-term health
- Transparent results with context ("combined with diet/exercise, 6 months")
- Educational value, meal ideas, workout demos, tracking tips
- Engagement: Q&A, community, realistic expectations

## Usage in Tool
- Parse sections for keyword lists.
- For chunk text: keyword match + embed similarity (cosine > 0.65 to rule phrases via `EmbeddingProvider`).
- Output in frontmatter: `topics: [list]`, `sentiment: positive`, `pros: [..]`, `cons: [..]`, `risk: true/false`, `risk_reason: "misleading claim"`.
- Refine this file based on real clips/tests.

**Sources**: YouTube Monetization Policies (1311392), Advertiser-Friendly Guidelines (6162278), Community Guidelines. Check official links for updates. Avoid hardcoding - keep rules here for easy iteration.

**TODO**: Expand with more examples from real videos, test thresholds with embeddings.
