"""Preserve Markdown artifact targets when text is copied to another directory."""
import os
from pathlib import Path
import re
from urllib.parse import quote, unquote, urlsplit, urlunsplit


def relocate_links(text, source, destination):
    source, destination = Path(source), Path(destination)
    if source.parent == destination.parent:
        return text

    def target(value):
        angled = value.startswith('<') and value.endswith('>')
        raw = value[1:-1] if angled else value
        parsed = urlsplit(raw)
        if parsed.scheme or parsed.netloc or not parsed.path or parsed.path.startswith('/'):
            return value
        resolved = (source.parent / unquote(parsed.path)).resolve()
        relative = Path(os.path.relpath(resolved, destination.parent)).as_posix()
        result = urlunsplit(('', '', quote(relative, safe='/.-_~'), parsed.query, parsed.fragment))
        return '<' + result + '>' if angled else result

    # Preserve fenced code and inline code. Handle ordinary inline/image links and definitions.
    chunks = re.split(r'(^```[^\n]*\n.*?^```[^\n]*$|^~~~[^\n]*\n.*?^~~~[^\n]*$|`[^`\n]*`)',
                      text, flags=re.M | re.S)
    for index in range(0, len(chunks), 2):
        chunk = re.sub(r'(\]\()(<[^>\n]+>|[^\s()<>]+)(?=\s+[\"\']|\))',
                       lambda m: m[1] + target(m[2]), chunks[index])
        chunks[index] = re.sub(r'(^ {0,3}\[[^\]\n]+\]:\s*)(<[^>\n]+>|\S+)',
                               lambda m: m[1] + target(m[2]), chunk, flags=re.M)
    return ''.join(chunks)
