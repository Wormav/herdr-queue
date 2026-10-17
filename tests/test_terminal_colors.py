from rich.segment import Segment
from rich.style import Style
from textual.color import Color

from herdr_drafts.terminal_colors import TerminalDefaultColors


def test_terminal_defaults_survive_textual_rich_style_conversion():
    native = Color.parse("ansi_default").rich_color
    converted = Color.from_rich_color(native).rich_color
    segments = [Segment("texte", Style(color=converted, bgcolor=converted, bold=True))]
    rendered = TerminalDefaultColors().apply(segments, Color.parse("ansi_default"))
    assert rendered[0].style.color.is_default
    assert rendered[0].style.bgcolor.is_default
    assert rendered[0].style.bold


def test_explicit_ansi_black_and_other_palette_colors_are_preserved():
    segments = [Segment("texte", Style(color="black", bgcolor="blue", reverse=True))]
    assert TerminalDefaultColors().apply(segments, Color.parse("ansi_default")) == segments
