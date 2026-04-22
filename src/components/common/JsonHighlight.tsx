/** JSON syntax highlighter — React nodes, no dangerouslySetInnerHTML */
export default function JsonHighlight({ json }: { json: string }) {
  const tokens = json.split(/("(?:\\.|[^"\\])*")/g);
  let nextIsValue = false;
  return (
    <>
      {tokens.map((tok, i) => {
        if (tok.startsWith('"')) {
          if (nextIsValue) {
            nextIsValue = false;
            return <span key={i} className="text-green-400">{tok}</span>;
          }
          return <span key={i} className="text-purple-400">{tok}</span>;
        }
        nextIsValue = tok.trimEnd().endsWith(':');
        const parts = tok.split(/\b(true|false|null|\d+\.?\d*(?:e[+-]?\d+)?)\b/gi);
        return (
          <span key={i}>
            {parts.map((p, j) => {
              if (/^(true|false)$/i.test(p)) return <span key={j} className="text-blue-400">{p}</span>;
              if (p === 'null') return <span key={j} className="text-gray-500">{p}</span>;
              if (/^\d/.test(p)) return <span key={j} className="text-amber-400">{p}</span>;
              return p;
            })}
          </span>
        );
      })}
    </>
  );
}
