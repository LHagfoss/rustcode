/*
 * Rounded icons (Hugeicons' free stroke-rounded set) under the Lucide names the app already
 * uses, so a file moves over by changing only its import (docs/design.md › Icons). They take
 * Lucide's props: `size`, `strokeWidth` (1.75 by default, like the app's Lucide icons),
 * `className`, `fill` and the other SVG attributes.
 */
import {
  AlertCircleIcon,
  ArrowDown01Icon,
  ArrowRight01Icon,
  ArrowTurnBackwardIcon,
  ArrowUp01Icon,
  ArrowUp02Icon,
  Attachment01Icon,
  BotIcon,
  Cancel01Icon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  CloudUploadIcon,
  CommandIcon,
  ComputerTerminal01Icon,
  Copy01Icon,
  DashedLineCircleIcon,
  Delete02Icon,
  File02Icon,
  FileDiffIcon,
  FilterHorizontalIcon,
  Folder01Icon,
  FolderAddIcon,
  FolderOpenIcon,
  FolderRemoveIcon,
  GitBranchIcon,
  GitForkIcon,
  GitMergeIcon,
  GitPullRequestClosedIcon,
  GitPullRequestDraftIcon,
  GitPullRequestIcon,
  LeftToRightListBulletIcon,
  MessageQuestionIcon,
  MoreHorizontalIcon,
  PencilEdit01Icon,
  PencilEdit02Icon,
  PinIcon,
  PinOffIcon,
  QuoteDownIcon,
  SecurityCheckIcon,
  Shield01Icon,
  ShieldBanIcon,
  ShieldQuestionMarkIcon,
  SidebarLeftIcon,
  SparklesIcon,
  Tick02Icon,
  ViewIcon,
  WifiOff01Icon,
  Wrench01Icon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon, type HugeiconsProps, type IconSvgElement } from "@hugeicons/react";

export type IconProps = Omit<HugeiconsProps, "icon" | "altIcon" | "showAlt" | "ref">;

function Rounded({ icon, strokeWidth = 1.75, ...props }: IconProps & { icon: IconSvgElement }) {
  return <HugeiconsIcon icon={icon} strokeWidth={strokeWidth} aria-hidden="true" {...props} />;
}

export const ArrowUp = (props: IconProps) => <Rounded icon={ArrowUp02Icon} {...props} />;
export const Bot = (props: IconProps) => <Rounded icon={BotIcon} {...props} />;
export const Check = (props: IconProps) => <Rounded icon={Tick02Icon} {...props} />;
export const ChevronDown = (props: IconProps) => <Rounded icon={ArrowDown01Icon} {...props} />;
export const ChevronRight = (props: IconProps) => <Rounded icon={ArrowRight01Icon} {...props} />;
export const ChevronUp = (props: IconProps) => <Rounded icon={ArrowUp01Icon} {...props} />;
export const CircleAlert = (props: IconProps) => <Rounded icon={AlertCircleIcon} {...props} />;
export const CircleCheck = (props: IconProps) => <Rounded icon={CheckmarkCircle02Icon} {...props} />;
export const CircleDashed = (props: IconProps) => <Rounded icon={DashedLineCircleIcon} {...props} />;
export const Clock = (props: IconProps) => <Rounded icon={Clock01Icon} {...props} />;
export const CloudUpload = (props: IconProps) => <Rounded icon={CloudUploadIcon} {...props} />;
export const Command = (props: IconProps) => <Rounded icon={CommandIcon} {...props} />;
export const Copy = (props: IconProps) => <Rounded icon={Copy01Icon} {...props} />;
export const CornerUpLeft = (props: IconProps) => <Rounded icon={ArrowTurnBackwardIcon} {...props} />;
export const Ellipsis = (props: IconProps) => <Rounded icon={MoreHorizontalIcon} {...props} />;
export const Eye = (props: IconProps) => <Rounded icon={ViewIcon} {...props} />;
export const FileDiff = (props: IconProps) => <Rounded icon={FileDiffIcon} {...props} />;
export const FileText = (props: IconProps) => <Rounded icon={File02Icon} {...props} />;
export const Folder = (props: IconProps) => <Rounded icon={Folder01Icon} {...props} />;
export const FolderOpen = (props: IconProps) => <Rounded icon={FolderOpenIcon} {...props} />;
export const FolderPlus = (props: IconProps) => <Rounded icon={FolderAddIcon} {...props} />;
export const FolderX = (props: IconProps) => <Rounded icon={FolderRemoveIcon} {...props} />;
export const GitBranch = (props: IconProps) => <Rounded icon={GitBranchIcon} {...props} />;
export const GitFork = (props: IconProps) => <Rounded icon={GitForkIcon} {...props} />;
export const GitMerge = (props: IconProps) => <Rounded icon={GitMergeIcon} {...props} />;
export const GitPullRequestArrow = (props: IconProps) => <Rounded icon={GitPullRequestIcon} {...props} />;
export const GitPullRequestClosed = (props: IconProps) => (
  <Rounded icon={GitPullRequestClosedIcon} {...props} />
);
export const GitPullRequestDraft = (props: IconProps) => (
  <Rounded icon={GitPullRequestDraftIcon} {...props} />
);
export const ListEnd = (props: IconProps) => <Rounded icon={LeftToRightListBulletIcon} {...props} />;
export const ListFilter = (props: IconProps) => <Rounded icon={FilterHorizontalIcon} {...props} />;
export const MessageCircleQuestion = (props: IconProps) => <Rounded icon={MessageQuestionIcon} {...props} />;
export const MessageSquareQuote = (props: IconProps) => <Rounded icon={QuoteDownIcon} {...props} />;
export const PanelLeft = (props: IconProps) => <Rounded icon={SidebarLeftIcon} {...props} />;
export const Paperclip = (props: IconProps) => <Rounded icon={Attachment01Icon} {...props} />;
export const Pencil = (props: IconProps) => <Rounded icon={PencilEdit01Icon} {...props} />;
export const Pin = (props: IconProps) => <Rounded icon={PinIcon} {...props} />;
export const PinOff = (props: IconProps) => <Rounded icon={PinOffIcon} {...props} />;
export const Shield = (props: IconProps) => <Rounded icon={Shield01Icon} {...props} />;
export const ShieldCheck = (props: IconProps) => <Rounded icon={SecurityCheckIcon} {...props} />;
export const ShieldOff = (props: IconProps) => <Rounded icon={ShieldBanIcon} {...props} />;
export const ShieldQuestion = (props: IconProps) => <Rounded icon={ShieldQuestionMarkIcon} {...props} />;
export const Sparkles = (props: IconProps) => <Rounded icon={SparklesIcon} {...props} />;
export const SquarePen = (props: IconProps) => <Rounded icon={PencilEdit02Icon} {...props} />;
export const SquareTerminal = (props: IconProps) => <Rounded icon={ComputerTerminal01Icon} {...props} />;
export const Trash2 = (props: IconProps) => <Rounded icon={Delete02Icon} {...props} />;
export const WifiOff = (props: IconProps) => <Rounded icon={WifiOff01Icon} {...props} />;
export const Wrench = (props: IconProps) => <Rounded icon={Wrench01Icon} {...props} />;
export const X = (props: IconProps) => <Rounded icon={Cancel01Icon} {...props} />;
