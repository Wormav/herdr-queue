"""Keep terminal defaults intact through Textual 6.6's Rich style conversions."""

from rich.color import ColorType
from rich.segment import Segment
from rich.style import Style
from textual.color import Color
from textual.filter import LineFilter


class TerminalDefaultColors(LineFilter):
    """Restore synthetic RGB black defaults without changing explicit ANSI colors."""

    def apply(self, segments: list[Segment], background: Color) -> list[Segment]:
        restored = []
        for segment in segments:
            style = segment.style
            if style is not None:
                colors = [
                    "default"
                    if color is not None
                    and color.type == ColorType.TRUECOLOR
                    and color.triplet == (0, 0, 0)
                    else None
                    for color in (style.color, style.bgcolor)
                ]
                if any(colors):
                    style += Style(color=colors[0], bgcolor=colors[1])
                    segment = Segment(segment.text, style, segment.control)
            restored.append(segment)
        return restored
