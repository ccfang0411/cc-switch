import type { SessionTurn } from "./turns";

const MAX_TURNS = 6;
const MAX_CHARS = 12_000;

interface ContextLabels {
  intro: string;
  question: string;
  answer: string;
  truncated: string;
}

export function buildKimiContinuationContext(
  turns: readonly SessionTurn[],
  labels: ContextLabels,
): string | null {
  const excerpts: string[] = [];
  let remaining = MAX_CHARS - labels.intro.length - labels.truncated.length - 4;
  let truncated = false;
  for (let index = turns.length - 1; index >= 0; index -= 1) {
    const turn = turns[index];
    const question = turn.question?.text.trim();
    const answer = turn.final?.text.trim();
    if (!question && !answer) continue;
    if (excerpts.length === MAX_TURNS) {
      truncated = true;
      break;
    }
    const excerpt = [
      question ? `${labels.question}:\n${question}` : "",
      answer ? `${labels.answer}:\n${answer}` : "",
    ]
      .filter(Boolean)
      .join("\n\n");
    if (excerpt.length > remaining) {
      if (excerpts.length === 0) {
        excerpts.push(
          excerpt.slice(0, remaining).replace(/[\uD800-\uDBFF]$/, ""),
        );
      }
      truncated = true;
      break;
    }
    excerpts.push(excerpt);
    remaining -= excerpt.length + 2;
  }
  if (excerpts.length === 0) return null;
  return [
    labels.intro,
    ...excerpts.reverse(),
    truncated ? labels.truncated : "",
  ]
    .filter(Boolean)
    .join("\n\n");
}
