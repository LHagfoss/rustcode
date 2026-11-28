import { inCode, type TokenRange } from "../state/compose";

const EMOJI = [
  ["😀", "grinning"],
  ["😄", "smile"],
  ["😆", "laughing", "xd satisfied"],
  ["😂", "joy", "laugh tears lol"],
  ["🤣", "rofl", "rolling on the floor laughing"],
  ["😊", "blush", "smile"],
  ["🙂", "slight_smile"],
  ["🙃", "upside_down"],
  ["😉", "wink"],
  ["😍", "heart_eyes", "love"],
  ["🥰", "smiling_face_with_hearts", "love"],
  ["😘", "kissing_heart"],
  ["😎", "sunglasses", "cool"],
  ["🤔", "thinking", "hmm"],
  ["🫡", "saluting_face"],
  ["🤫", "shushing_face"],
  ["😅", "sweat_smile"],
  ["😇", "innocent", "halo"],
  ["🤩", "star_struck"],
  ["😋", "yum"],
  ["😜", "stuck_out_tongue_winking_eye"],
  ["🤪", "zany_face"],
  ["🥹", "holding_back_tears"],
  ["🥺", "pleading_face"],
  ["😢", "cry", "sad"],
  ["😭", "sob", "cry"],
  ["😬", "grimacing"],
  ["😳", "flushed"],
  ["😡", "rage", "angry"],
  ["🤯", "exploding_head", "mind blown"],
  ["💀", "skull"],
  ["👀", "eyes", "look"],
  ["👍", "thumbsup", "+1 like thumbs_up"],
  ["👎", "thumbsdown", "-1 dislike thumbs_down"],
  ["👏", "clap"],
  ["🙌", "raised_hands", "hooray"],
  ["🙏", "pray", "thanks please folded hands"],
  ["👋", "wave", "hello goodbye"],
  ["🤝", "handshake", "deal"],
  ["💪", "muscle", "strong flex"],
  ["👌", "ok_hand"],
  ["✌️", "v", "peace victory"],
  ["🤞", "crossed_fingers", "luck"],
  ["❤️", "heart", "love red_heart"],
  ["💙", "blue_heart"],
  ["💚", "green_heart"],
  ["💛", "yellow_heart"],
  ["💜", "purple_heart"],
  ["🖤", "black_heart"],
  ["🤍", "white_heart"],
  ["💔", "broken_heart"],
  ["🔥", "fire", "lit"],
  ["✨", "sparkles"],
  ["⭐", "star"],
  ["🎉", "tada", "party celebrate confetti"],
  ["🥳", "partying_face", "celebrate"],
  ["🚀", "rocket", "ship launch"],
  ["✅", "white_check_mark", "done check"],
  ["❌", "x", "cross no"],
  ["⚠️", "warning"],
  ["💯", "100", "hundred"],
  ["💡", "bulb", "idea"],
  ["🐛", "bug"],
  ["🐱", "cat"],
  ["🐶", "dog"],
  ["🦀", "crab", "rust"],
  ["🦊", "fox"],
  ["☕", "coffee"],
] as const;

export function emojiToken(text: string, caret: number): TokenRange | null {
  const before = text.slice(0, caret);
  const match = /(?:^|\s):([a-z0-9_+-]*)$/i.exec(before);
  if (!match || /^[a-z0-9_+:-]/i.test(text.slice(caret))) return null;
  const query = match[1] ?? "";
  const start = caret - query.length - 1;
  return inCode(text, start) ? null : { start, end: caret, query };
}

export function emojiMatches(query: string) {
  const search = query.toLowerCase();
  const prefix = [];
  const other = [];
  for (const [glyph, name, aliases = ""] of EMOJI) {
    const emoji = { glyph, name };
    if (name === search) prefix.unshift(emoji);
    else if (name.startsWith(search)) prefix.push(emoji);
    else if (name.includes(search) || aliases.includes(search)) other.push(emoji);
  }
  return [...prefix, ...other];
}
