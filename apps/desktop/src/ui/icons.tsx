/*
 * The app's icons: Hugeicons' free stroke-rounded set under the Lucide names the code already
 * uses (docs/design.md › Icons). Import every icon from here, never from `lucide-react`. They take
 * Lucide's props: `size`, `strokeWidth` (1.5 by default, the set's own weight), `className`,
 * `fill` and the other SVG attributes. The few shapes the set has no match for (the raised hand,
 * a dotted circle, the sun-and-moon, a branch with a plus) stay Lucide, drawn at the same weight.
 */
import {
  Activity01Icon,
  AddCircleIcon,
  Alert02Icon,
  AlertCircleIcon,
  Analytics01Icon,
  Archive03Icon,
  ArrowDataTransferVerticalIcon,
  ArrowDown01Icon,
  ArrowDown02Icon,
  ArrowLeft01Icon,
  ArrowLeft02Icon,
  ArrowMoveDownRightIcon,
  ArrowRight01Icon,
  ArrowRight02Icon,
  ArrowTurnBackwardIcon,
  ArrowUp01Icon,
  ArrowUp02Icon,
  ArrowUpRight01Icon,
  Attachment01Icon,
  Book01Icon,
  BookOpen01Icon,
  BotIcon,
  Brain02Icon,
  Bug01Icon,
  Calendar03Icon,
  Cancel01Icon,
  CancelCircleIcon,
  ChartColumnIcon,
  CheckListIcon,
  CheckmarkCircle02Icon,
  Chemistry01Icon,
  CircleArrowUp01Icon,
  ClipboardPasteIcon,
  Clock01Icon,
  CloudIcon,
  CloudUploadIcon,
  CommandIcon,
  CommandLineIcon,
  ComputerIcon,
  ComputerTerminal01Icon,
  Copy01Icon,
  DashboardSpeed01Icon,
  DashedLineCircleIcon,
  Delete02Icon,
  Download01Icon,
  DragDropVerticalIcon,
  Edit03Icon,
  File02Icon,
  FileAddIcon,
  FileDiffIcon,
  FilterHorizontalIcon,
  FilterIcon,
  Folder01Icon,
  FolderAddIcon,
  FolderGitTwoIcon,
  FolderOpenIcon,
  FolderRemoveIcon,
  GitBranchIcon,
  GitCommitHorizontalIcon,
  GitCompareIcon,
  GitForkIcon,
  GitMergeIcon,
  GitPullRequestClosedIcon,
  GitPullRequestDraftIcon,
  GitPullRequestIcon,
  Globe02Icon,
  GridViewIcon,
  HelpCircleIcon,
  HierarchyIcon,
  Home01Icon,
  HourglassIcon,
  Image01Icon,
  ImageNotFound01Icon,
  InformationCircleIcon,
  Key01Icon,
  KeyboardIcon,
  LaptopIcon,
  Layers01Icon,
  LayoutThreeColumnIcon,
  LayoutThreeRowIcon,
  LeftToRightListBulletIcon,
  LinkSquare02Icon,
  ListViewIcon,
  Loading03Icon,
  Maximize01Icon,
  Message01Icon,
  Message02Icon,
  MessageAdd01Icon,
  MessageMultiple01Icon,
  MessageQuestionIcon,
  Mic01Icon,
  MicOff01Icon,
  Minimize01Icon,
  MinusSignIcon,
  Moon02Icon,
  MoreHorizontalIcon,
  MoveRightIcon,
  PackageIcon,
  PaintBoardIcon,
  PaintBrush01Icon,
  PauseCircleIcon,
  PauseIcon,
  PencilEdit02Icon,
  PinIcon,
  PinOffIcon,
  PlayIcon,
  Plug01Icon,
  PlusSignIcon,
  PuzzleIcon,
  QuoteDownIcon,
  RefreshIcon,
  RepeatIcon,
  Search01Icon,
  SecurityCheckIcon,
  SecurityWarningIcon,
  SentIcon,
  ServerStack01Icon,
  Settings01Icon,
  Settings02Icon,
  Shield01Icon,
  ShieldBanIcon,
  ShieldQuestionMarkIcon,
  SidebarLeftIcon,
  SidebarRightIcon,
  SlidersHorizontalIcon,
  SmartPhone01Icon,
  SourceCodeIcon,
  SparklesIcon,
  SquareIcon,
  SquareLock02Icon,
  StarIcon,
  Stethoscope02Icon,
  StopIcon,
  Sun03Icon,
  Tag01Icon,
  Tick02Icon,
  TickDouble02Icon,
  Time04Icon,
  ToolsIcon,
  UnavailableIcon,
  Undo02Icon,
  Unlink01Icon,
  UserIcon,
  UserMultipleIcon,
  ViewIcon,
  ViewOffIcon,
  WebhookIcon,
  WifiOff01Icon,
  WorkflowSquare01Icon,
  Wrench01Icon,
  ZapIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon, type HugeiconsProps, type IconSvgElement } from "@hugeicons/react";
import {
  CircleDot as LucideCircleDot,
  GitBranchPlus as LucideGitBranchPlus,
  Hand as LucideHand,
  type LucideProps,
  SunMoon as LucideSunMoon,
} from "lucide-react";
import type { ComponentType } from "react";

export type IconProps = Omit<HugeiconsProps, "icon" | "altIcon" | "showAlt" | "ref">;
/** Any icon from this file, for props and tables that take one. */
export type LucideIcon = ComponentType<IconProps>;

const STROKE = 1.5;

function Rounded({
  name,
  icon,
  strokeWidth = STROKE,
  ...props
}: IconProps & { name: string; icon: IconSvgElement }) {
  return (
    <HugeiconsIcon icon={icon} strokeWidth={strokeWidth} aria-hidden="true" data-icon={name} {...props} />
  );
}

function Lucide({
  name,
  glyph: Glyph,
  strokeWidth = STROKE,
  ...props
}: IconProps & { name: string; glyph: ComponentType<LucideProps> }) {
  return <Glyph strokeWidth={strokeWidth} aria-hidden="true" data-icon={name} {...(props as LucideProps)} />;
}

export const Activity = (props: IconProps) => <Rounded name="Activity" icon={Activity01Icon} {...props} />;
export const Archive = (props: IconProps) => <Rounded name="Archive" icon={Archive03Icon} {...props} />;
export const ArrowDown = (props: IconProps) => <Rounded name="ArrowDown" icon={ArrowDown02Icon} {...props} />;
export const ArrowDownUp = (props: IconProps) => (
  <Rounded name="ArrowDownUp" icon={ArrowDataTransferVerticalIcon} {...props} />
);
export const ArrowLeft = (props: IconProps) => <Rounded name="ArrowLeft" icon={ArrowLeft02Icon} {...props} />;
export const ArrowRight = (props: IconProps) => (
  <Rounded name="ArrowRight" icon={ArrowRight02Icon} {...props} />
);
export const ArrowUp = (props: IconProps) => <Rounded name="ArrowUp" icon={ArrowUp02Icon} {...props} />;
export const ArrowUpCircle = (props: IconProps) => (
  <Rounded name="ArrowUpCircle" icon={CircleArrowUp01Icon} strokeWidth={2} {...props} />
);
export const ArrowUpRight = (props: IconProps) => (
  <Rounded name="ArrowUpRight" icon={ArrowUpRight01Icon} {...props} />
);
export const BarChart3 = (props: IconProps) => <Rounded name="BarChart3" icon={Analytics01Icon} {...props} />;
export const Blocks = (props: IconProps) => <Rounded name="Blocks" icon={PuzzleIcon} {...props} />;
export const Book = (props: IconProps) => <Rounded name="Book" icon={Book01Icon} {...props} />;
export const BookOpen = (props: IconProps) => <Rounded name="BookOpen" icon={BookOpen01Icon} {...props} />;
export const Bot = (props: IconProps) => <Rounded name="Bot" icon={BotIcon} {...props} />;
export const Box = (props: IconProps) => <Rounded name="Box" icon={PackageIcon} {...props} />;
export const Brain = (props: IconProps) => <Rounded name="Brain" icon={Brain02Icon} {...props} />;
export const Bug = (props: IconProps) => <Rounded name="Bug" icon={Bug01Icon} {...props} />;
export const CalendarDays = (props: IconProps) => (
  <Rounded name="CalendarDays" icon={Calendar03Icon} {...props} />
);
export const ChartColumn = (props: IconProps) => (
  <Rounded name="ChartColumn" icon={ChartColumnIcon} {...props} />
);
export const Check = (props: IconProps) => <Rounded name="Check" icon={Tick02Icon} {...props} />;
export const CheckCheck = (props: IconProps) => (
  <Rounded name="CheckCheck" icon={TickDouble02Icon} {...props} />
);
export const ChevronDown = (props: IconProps) => (
  <Rounded name="ChevronDown" icon={ArrowDown01Icon} {...props} />
);
export const ChevronLeft = (props: IconProps) => (
  <Rounded name="ChevronLeft" icon={ArrowLeft01Icon} {...props} />
);
export const ChevronRight = (props: IconProps) => (
  <Rounded name="ChevronRight" icon={ArrowRight01Icon} {...props} />
);
export const ChevronUp = (props: IconProps) => <Rounded name="ChevronUp" icon={ArrowUp01Icon} {...props} />;
export const CircleAlert = (props: IconProps) => (
  <Rounded name="CircleAlert" icon={AlertCircleIcon} {...props} />
);
export const CircleArrowUp = (props: IconProps) => (
  <Rounded name="CircleArrowUp" icon={CircleArrowUp01Icon} {...props} />
);
export const CircleCheck = (props: IconProps) => (
  <Rounded name="CircleCheck" icon={CheckmarkCircle02Icon} {...props} />
);
export const CircleDashed = (props: IconProps) => (
  <Rounded name="CircleDashed" icon={DashedLineCircleIcon} {...props} />
);
export const CircleHelp = (props: IconProps) => (
  <Rounded name="CircleHelp" icon={HelpCircleIcon} {...props} />
);
export const CirclePlus = (props: IconProps) => <Rounded name="CirclePlus" icon={AddCircleIcon} {...props} />;
export const CircleSlash = (props: IconProps) => (
  <Rounded name="CircleSlash" icon={UnavailableIcon} {...props} />
);
export const CircleX = (props: IconProps) => <Rounded name="CircleX" icon={CancelCircleIcon} {...props} />;
export const ClipboardPaste = (props: IconProps) => (
  <Rounded name="ClipboardPaste" icon={ClipboardPasteIcon} {...props} />
);
export const Clock = (props: IconProps) => <Rounded name="Clock" icon={Clock01Icon} {...props} />;
export const Cloud = (props: IconProps) => <Rounded name="Cloud" icon={CloudIcon} {...props} />;
export const CloudUpload = (props: IconProps) => (
  <Rounded name="CloudUpload" icon={CloudUploadIcon} {...props} />
);
export const Code = (props: IconProps) => <Rounded name="Code" icon={SourceCodeIcon} {...props} />;
export const Columns3 = (props: IconProps) => (
  <Rounded name="Columns3" icon={LayoutThreeColumnIcon} {...props} />
);
export const Command = (props: IconProps) => <Rounded name="Command" icon={CommandIcon} {...props} />;
export const Computer = (props: IconProps) => <Rounded name="Computer" icon={ComputerIcon} {...props} />;
export const Copy = (props: IconProps) => <Rounded name="Copy" icon={Copy01Icon} {...props} />;
export const CornerDownLeft = (props: IconProps) => (
  <Rounded name="CornerDownLeft" icon={ArrowTurnBackwardIcon} {...props} />
);
export const CornerDownRight = (props: IconProps) => (
  <Rounded name="CornerDownRight" icon={ArrowMoveDownRightIcon} {...props} />
);
export const CornerUpLeft = (props: IconProps) => (
  <Rounded name="CornerUpLeft" icon={ArrowTurnBackwardIcon} {...props} />
);
export const Download = (props: IconProps) => <Rounded name="Download" icon={Download01Icon} {...props} />;
export const Ellipsis = (props: IconProps) => (
  <Rounded name="Ellipsis" icon={MoreHorizontalIcon} {...props} />
);
export const ExternalLink = (props: IconProps) => (
  <Rounded name="ExternalLink" icon={LinkSquare02Icon} {...props} />
);
export const Eye = (props: IconProps) => <Rounded name="Eye" icon={ViewIcon} {...props} />;
export const EyeOff = (props: IconProps) => <Rounded name="EyeOff" icon={ViewOffIcon} {...props} />;
export const FileDiff = (props: IconProps) => <Rounded name="FileDiff" icon={FileDiffIcon} {...props} />;
export const FilePlus = (props: IconProps) => <Rounded name="FilePlus" icon={FileAddIcon} {...props} />;
export const FilePlus2 = (props: IconProps) => <Rounded name="FilePlus2" icon={FileAddIcon} {...props} />;
export const FileText = (props: IconProps) => <Rounded name="FileText" icon={File02Icon} {...props} />;
export const Filter = (props: IconProps) => <Rounded name="Filter" icon={FilterIcon} {...props} />;
export const FlaskConical = (props: IconProps) => (
  <Rounded name="FlaskConical" icon={Chemistry01Icon} {...props} />
);
export const Folder = (props: IconProps) => <Rounded name="Folder" icon={Folder01Icon} {...props} />;
export const FolderGit2 = (props: IconProps) => (
  <Rounded name="FolderGit2" icon={FolderGitTwoIcon} {...props} />
);
export const FolderOpen = (props: IconProps) => (
  <Rounded name="FolderOpen" icon={FolderOpenIcon} {...props} />
);
export const FolderPlus = (props: IconProps) => <Rounded name="FolderPlus" icon={FolderAddIcon} {...props} />;
export const FolderX = (props: IconProps) => <Rounded name="FolderX" icon={FolderRemoveIcon} {...props} />;
export const Gauge = (props: IconProps) => <Rounded name="Gauge" icon={DashboardSpeed01Icon} {...props} />;
export const GitBranch = (props: IconProps) => <Rounded name="GitBranch" icon={GitBranchIcon} {...props} />;
export const GitCommitHorizontal = (props: IconProps) => (
  <Rounded name="GitCommitHorizontal" icon={GitCommitHorizontalIcon} {...props} />
);
export const GitCompareArrows = (props: IconProps) => (
  <Rounded name="GitCompareArrows" icon={GitCompareIcon} {...props} />
);
export const GitFork = (props: IconProps) => <Rounded name="GitFork" icon={GitForkIcon} {...props} />;
export const GitMerge = (props: IconProps) => <Rounded name="GitMerge" icon={GitMergeIcon} {...props} />;
export const GitPullRequest = (props: IconProps) => (
  <Rounded name="GitPullRequest" icon={GitPullRequestIcon} {...props} />
);
export const GitPullRequestArrow = (props: IconProps) => (
  <Rounded name="GitPullRequestArrow" icon={GitPullRequestIcon} {...props} />
);
export const GitPullRequestClosed = (props: IconProps) => (
  <Rounded name="GitPullRequestClosed" icon={GitPullRequestClosedIcon} {...props} />
);
export const GitPullRequestDraft = (props: IconProps) => (
  <Rounded name="GitPullRequestDraft" icon={GitPullRequestDraftIcon} {...props} />
);
export const Globe = (props: IconProps) => <Rounded name="Globe" icon={Globe02Icon} {...props} />;
export const GripVertical = (props: IconProps) => (
  <Rounded name="GripVertical" icon={DragDropVerticalIcon} {...props} />
);
export const Hammer = (props: IconProps) => <Rounded name="Hammer" icon={ToolsIcon} {...props} />;
export const History = (props: IconProps) => <Rounded name="History" icon={Time04Icon} {...props} />;
export const Hourglass = (props: IconProps) => <Rounded name="Hourglass" icon={HourglassIcon} {...props} />;
export const House = (props: IconProps) => <Rounded name="House" icon={Home01Icon} {...props} />;
export const Image = (props: IconProps) => <Rounded name="Image" icon={Image01Icon} {...props} />;
export const ImageOff = (props: IconProps) => (
  <Rounded name="ImageOff" icon={ImageNotFound01Icon} {...props} />
);
export const Info = (props: IconProps) => <Rounded name="Info" icon={InformationCircleIcon} {...props} />;
export const KeyRound = (props: IconProps) => <Rounded name="KeyRound" icon={Key01Icon} {...props} />;
export const Keyboard = (props: IconProps) => <Rounded name="Keyboard" icon={KeyboardIcon} {...props} />;
export const Laptop = (props: IconProps) => <Rounded name="Laptop" icon={LaptopIcon} {...props} />;
export const Layers = (props: IconProps) => <Rounded name="Layers" icon={Layers01Icon} {...props} />;
export const LayoutGrid = (props: IconProps) => <Rounded name="LayoutGrid" icon={GridViewIcon} {...props} />;
export const List = (props: IconProps) => <Rounded name="List" icon={ListViewIcon} {...props} />;
export const ListChecks = (props: IconProps) => <Rounded name="ListChecks" icon={CheckListIcon} {...props} />;
export const ListEnd = (props: IconProps) => (
  <Rounded name="ListEnd" icon={LeftToRightListBulletIcon} {...props} />
);
export const ListFilter = (props: IconProps) => (
  <Rounded name="ListFilter" icon={FilterHorizontalIcon} {...props} />
);
export const ListTree = (props: IconProps) => <Rounded name="ListTree" icon={HierarchyIcon} {...props} />;
export const Loader2 = (props: IconProps) => <Rounded name="Loader2" icon={Loading03Icon} {...props} />;
export const LoaderCircle = (props: IconProps) => (
  <Rounded name="LoaderCircle" icon={Loading03Icon} {...props} />
);
export const Lock = (props: IconProps) => <Rounded name="Lock" icon={SquareLock02Icon} {...props} />;
export const Maximize2 = (props: IconProps) => <Rounded name="Maximize2" icon={Maximize01Icon} {...props} />;
export const MessageCircleQuestion = (props: IconProps) => (
  <Rounded name="MessageCircleQuestion" icon={MessageQuestionIcon} {...props} />
);
export const MessageSquare = (props: IconProps) => (
  <Rounded name="MessageSquare" icon={Message01Icon} {...props} />
);
export const MessageSquarePlus = (props: IconProps) => (
  <Rounded name="MessageSquarePlus" icon={MessageAdd01Icon} {...props} />
);
export const MessageSquareQuote = (props: IconProps) => (
  <Rounded name="MessageSquareQuote" icon={QuoteDownIcon} {...props} />
);
export const MessageSquareText = (props: IconProps) => (
  <Rounded name="MessageSquareText" icon={Message02Icon} {...props} />
);
export const MessagesSquare = (props: IconProps) => (
  <Rounded name="MessagesSquare" icon={MessageMultiple01Icon} {...props} />
);
export const Mic = (props: IconProps) => <Rounded name="Mic" icon={Mic01Icon} {...props} />;
export const MicOff = (props: IconProps) => <Rounded name="MicOff" icon={MicOff01Icon} {...props} />;
export const Minimize2 = (props: IconProps) => <Rounded name="Minimize2" icon={Minimize01Icon} {...props} />;
export const Minus = (props: IconProps) => <Rounded name="Minus" icon={MinusSignIcon} {...props} />;
export const Monitor = (props: IconProps) => <Rounded name="Monitor" icon={ComputerIcon} {...props} />;
export const Moon = (props: IconProps) => <Rounded name="Moon" icon={Moon02Icon} {...props} />;
export const MoreHorizontal = (props: IconProps) => (
  <Rounded name="MoreHorizontal" icon={MoreHorizontalIcon} {...props} />
);
export const MoveRight = (props: IconProps) => <Rounded name="MoveRight" icon={MoveRightIcon} {...props} />;
export const Network = (props: IconProps) => <Rounded name="Network" icon={HierarchyIcon} {...props} />;
export const Palette = (props: IconProps) => <Rounded name="Palette" icon={PaintBoardIcon} {...props} />;
export const Paintbrush = (props: IconProps) => (
  <Rounded name="Paintbrush" icon={PaintBrush01Icon} {...props} />
);
export const PanelLeft = (props: IconProps) => <Rounded name="PanelLeft" icon={SidebarLeftIcon} {...props} />;
export const PanelRight = (props: IconProps) => (
  <Rounded name="PanelRight" icon={SidebarRightIcon} {...props} />
);
export const Paperclip = (props: IconProps) => (
  <Rounded name="Paperclip" icon={Attachment01Icon} {...props} />
);
export const Pause = (props: IconProps) => <Rounded name="Pause" icon={PauseIcon} {...props} />;
export const PauseCircle = (props: IconProps) => (
  <Rounded name="PauseCircle" icon={PauseCircleIcon} {...props} />
);
export const Pencil = (props: IconProps) => <Rounded name="Pencil" icon={Edit03Icon} {...props} />;
export const Pin = (props: IconProps) => <Rounded name="Pin" icon={PinIcon} {...props} />;
export const PinOff = (props: IconProps) => <Rounded name="PinOff" icon={PinOffIcon} {...props} />;
export const Play = (props: IconProps) => <Rounded name="Play" icon={PlayIcon} {...props} />;
export const Plug = (props: IconProps) => <Rounded name="Plug" icon={Plug01Icon} {...props} />;
export const Plus = (props: IconProps) => <Rounded name="Plus" icon={PlusSignIcon} {...props} />;
export const RefreshCw = (props: IconProps) => (
  <Rounded name="RefreshCw" icon={RefreshIcon} strokeWidth={2} {...props} />
);
// Reset, restart, retry and refresh are one glyph, the arrow circle: three near-identical arrows
// read as three different actions.
export const RotateCcw = (props: IconProps) => (
  <Rounded name="RotateCcw" icon={RefreshIcon} strokeWidth={2} {...props} />
);
export const Repeat = (props: IconProps) => <Rounded name="Repeat" icon={RepeatIcon} {...props} />;
export const RotateCw = (props: IconProps) => <Rounded name="RotateCw" icon={RefreshIcon} {...props} />;
export const Rows3 = (props: IconProps) => <Rounded name="Rows3" icon={LayoutThreeRowIcon} {...props} />;
export const Search = (props: IconProps) => <Rounded name="Search" icon={Search01Icon} {...props} />;
export const Send = (props: IconProps) => <Rounded name="Send" icon={SentIcon} {...props} />;
export const Server = (props: IconProps) => <Rounded name="Server" icon={ServerStack01Icon} {...props} />;
export const Settings = (props: IconProps) => <Rounded name="Settings" icon={Settings01Icon} {...props} />;
export const Settings2 = (props: IconProps) => <Rounded name="Settings2" icon={Settings02Icon} {...props} />;
export const Shield = (props: IconProps) => <Rounded name="Shield" icon={Shield01Icon} {...props} />;
export const ShieldAlert = (props: IconProps) => (
  <Rounded name="ShieldAlert" icon={SecurityWarningIcon} {...props} />
);
export const ShieldCheck = (props: IconProps) => (
  <Rounded name="ShieldCheck" icon={SecurityCheckIcon} {...props} />
);
export const ShieldOff = (props: IconProps) => <Rounded name="ShieldOff" icon={ShieldBanIcon} {...props} />;
export const ShieldQuestion = (props: IconProps) => (
  <Rounded name="ShieldQuestion" icon={ShieldQuestionMarkIcon} {...props} />
);
export const SlidersHorizontal = (props: IconProps) => (
  <Rounded name="SlidersHorizontal" icon={SlidersHorizontalIcon} {...props} />
);
export const Smartphone = (props: IconProps) => (
  <Rounded name="Smartphone" icon={SmartPhone01Icon} {...props} />
);
export const Sparkles = (props: IconProps) => <Rounded name="Sparkles" icon={SparklesIcon} {...props} />;
export const Square = (props: IconProps) => <Rounded name="Square" icon={SquareIcon} {...props} />;
export const SquarePen = (props: IconProps) => (
  <Rounded name="SquarePen" icon={PencilEdit02Icon} {...props} />
);
export const SquareStop = (props: IconProps) => <Rounded name="SquareStop" icon={StopIcon} {...props} />;
export const SquareTerminal = (props: IconProps) => (
  <Rounded name="SquareTerminal" icon={ComputerTerminal01Icon} {...props} />
);
export const Star = (props: IconProps) => <Rounded name="Star" icon={StarIcon} {...props} />;
export const Stethoscope = (props: IconProps) => (
  <Rounded name="Stethoscope" icon={Stethoscope02Icon} {...props} />
);
export const Sun = (props: IconProps) => <Rounded name="Sun" icon={Sun03Icon} {...props} />;
export const Tag = (props: IconProps) => <Rounded name="Tag" icon={Tag01Icon} {...props} />;
export const Terminal = (props: IconProps) => <Rounded name="Terminal" icon={CommandLineIcon} {...props} />;
export const TerminalSquare = (props: IconProps) => (
  <Rounded name="TerminalSquare" icon={ComputerTerminal01Icon} {...props} />
);
export const Trash2 = (props: IconProps) => <Rounded name="Trash2" icon={Delete02Icon} {...props} />;
export const TriangleAlert = (props: IconProps) => (
  <Rounded name="TriangleAlert" icon={Alert02Icon} {...props} />
);
export const Undo2 = (props: IconProps) => <Rounded name="Undo2" icon={Undo02Icon} {...props} />;
export const Unplug = (props: IconProps) => <Rounded name="Unplug" icon={Unlink01Icon} {...props} />;
export const UserRound = (props: IconProps) => <Rounded name="UserRound" icon={UserIcon} {...props} />;
export const Users = (props: IconProps) => <Rounded name="Users" icon={UserMultipleIcon} {...props} />;
export const Webhook = (props: IconProps) => <Rounded name="Webhook" icon={WebhookIcon} {...props} />;
export const WifiOff = (props: IconProps) => <Rounded name="WifiOff" icon={WifiOff01Icon} {...props} />;
export const Workflow = (props: IconProps) => (
  <Rounded name="Workflow" icon={WorkflowSquare01Icon} {...props} />
);
export const Wrench = (props: IconProps) => <Rounded name="Wrench" icon={Wrench01Icon} {...props} />;
export const X = (props: IconProps) => <Rounded name="X" icon={Cancel01Icon} {...props} />;
export const Zap = (props: IconProps) => <Rounded name="Zap" icon={ZapIcon} {...props} />;
export const CircleDot = (props: IconProps) => <Lucide name="CircleDot" glyph={LucideCircleDot} {...props} />;
export const GitBranchPlus = (props: IconProps) => (
  <Lucide name="GitBranchPlus" glyph={LucideGitBranchPlus} {...props} />
);
export const Hand = (props: IconProps) => <Lucide name="Hand" glyph={LucideHand} {...props} />;
export const SunMoon = (props: IconProps) => <Lucide name="SunMoon" glyph={LucideSunMoon} {...props} />;
