import React, { useState, useEffect, useMemo } from 'react';
import {
    ArrowLeft, Save, FileJson, LayoutTemplate, RefreshCcw,
    CheckSquare, Square, Info, Settings, Shield, Clock,
    AlertCircle, Zap, Globe, Play
} from 'lucide-react';
import { useTranslation } from 'react-i18next';
// 引入 CodeMirror 6 相关组件
import CodeMirror from '@uiw/react-codemirror';
import { json } from '@codemirror/lang-json';
import { vscodeDark } from '@uiw/codemirror-theme-vscode';

import { Button, Select } from '../../../components';
import { useToast } from '../../../components/Toast';
import './LevelDatEditor.css';

interface LevelDatEditorProps {
    data: any;
    fileName: string;
    onBack: () => void;
    onSave: (newData: any) => Promise<void>;
    onLaunch?: () => void; // 可选的启动回调
}

type ViewMode = 'form' | 'json';

// --- NBT 辅助函数 (根节点) ---
const getVal = (root: any, key: string, defaultVal: any = '') => {
    if (!root?.Compound?.[key]) return defaultVal;
    const node = root.Compound[key];
    const typeKey = Object.keys(node)[0];
    return node[typeKey];
};

const setVal = (root: any, key: string, type: string, value: any) => {
    const newRoot = root ? JSON.parse(JSON.stringify(root)) : { Compound: {} };
    if (!newRoot.Compound) newRoot.Compound = {};
    let finalValue = value;
    if (type === 'Byte' || type === 'Short' || type === 'Int') finalValue = Number(value);
    else if (type === 'Long') finalValue = Number(value);
    else if (type === 'Float' || type === 'Double') finalValue = parseFloat(value);
    newRoot.Compound[key] = { [type]: finalValue };
    return newRoot;
};

// --- NBT 辅助函数 (Abilities 专用) ---
const getAbilityVal = (root: any, key: string, defaultVal: any = 0) => {
    const node = root?.Compound?.abilities?.Compound?.[key];
    if (!node) return defaultVal;
    const typeKey = Object.keys(node)[0];
    return node[typeKey];
};

const getVersionStr = (root: any, key: string) => {
    if (!root?.Compound?.[key]?.List) return "1.0.0.0.0";
    const list = root.Compound[key].List;
    if (!Array.isArray(list)) return "1.0.0.0.0";
    return list.map((item: any) => item.Int ?? 0).join('.');
};

const setVersionStr = (root: any, key: string, valueStr: string) => {
    const newRoot = root ? JSON.parse(JSON.stringify(root)) : { Compound: {} };
    if (!newRoot.Compound) newRoot.Compound = {};
    const parts = valueStr.split('.').map(s => parseInt(s.trim()) || 0);
    while (parts.length < 5) parts.push(0);
    const listData = parts.map(num => ({ Int: num }));
    newRoot.Compound[key] = { List: listData };
    return newRoot;
};

export const LevelDatEditor: React.FC<LevelDatEditorProps> = ({
                                                                  data,
                                                                  fileName,
                                                                  onBack,
                                                                  onSave,
                                                                  onLaunch
                                                              }) => {
    const [currentData, setCurrentData] = useState<any>(data || { Compound: {} });
    const [mode, setMode] = useState<ViewMode>('form');
    const [jsonString, setJsonString] = useState('');
    const [isSaving, setIsSaving] = useState(false);
    const [editorMarkers, setEditorMarkers] = useState<any[]>([]);
    const toast = useToast();
    const { t } = useTranslation();

    const GAME_TYPE_OPTIONS = useMemo(() => ([
        { label: t('LevelDatEditor.game_type_survival'), value: 0 },
        { label: t('LevelDatEditor.game_type_creative'), value: 1 },
        { label: t('LevelDatEditor.game_type_adventure'), value: 2 },
        { label: t('LevelDatEditor.game_type_spectator'), value: 6 }
    ]), [t]);

    const DIFFICULTY_OPTIONS = useMemo(() => ([
        { label: t('LevelDatEditor.difficulty_peaceful'), value: 0 },
        { label: t('LevelDatEditor.difficulty_easy'), value: 1 },
        { label: t('LevelDatEditor.difficulty_normal'), value: 2 },
        { label: t('LevelDatEditor.difficulty_hard'), value: 3 }
    ]), [t]);

    const GENERATOR_OPTIONS = useMemo(() => ([
        { label: t('LevelDatEditor.generator_old'), value: 0 },
        { label: t('LevelDatEditor.generator_infinite'), value: 1 },
        { label: t('LevelDatEditor.generator_flat'), value: 2 }
    ]), [t]);

    useEffect(() => {
        if (data && typeof data === 'object') setCurrentData(data);
    }, [data]);

    useEffect(() => {
        if (currentData) setJsonString(JSON.stringify(currentData, null, 4));
    }, [currentData]);

    const handleCodeMirrorChange = React.useCallback((value: string) => {
        setJsonString(value);
        try {
            if (value.trim()) {
                JSON.parse(value);
                setEditorMarkers([]);
            }
        } catch (e) {
            setEditorMarkers([e]);
        }
    }, []);

    const handleModeSwitch = (newMode: ViewMode) => {
        if (newMode === 'form') {
            if (editorMarkers.length > 0) {
                toast.error(t("LevelDatEditor.errors.json_syntax_block"));
                return;
            }
            try {
                const currentVal = jsonString;
                if (!currentVal.trim()) throw new Error(t("LevelDatEditor.errors.empty_content"));
                const parsed = JSON.parse(currentVal);
                if (!parsed?.Compound) throw new Error(t("LevelDatEditor.errors.missing_compound"));
                setCurrentData(parsed);
                setMode('form');
            } catch (e: any) {
                toast.error(t("LevelDatEditor.errors.format_error", { message: e.message }));
            }
        } else {
            setJsonString(JSON.stringify(currentData, null, 4));
            setEditorMarkers([]);
            setMode('json');
        }
    };

    const handleChange = (key: string, type: string, value: any) => {
        setCurrentData(setVal(currentData, key, type, value));
    };

    const handleAbilityChange = (key: string, type: string, value: any) => {
        const newRoot = JSON.parse(JSON.stringify(currentData));
        if (!newRoot.Compound) newRoot.Compound = {};
        if (!newRoot.Compound.abilities) newRoot.Compound.abilities = { Compound: {} };
        if (!newRoot.Compound.abilities.Compound) newRoot.Compound.abilities.Compound = {};

        let finalValue = value;
        if (type === 'Byte') finalValue = Number(value);
        else if (type === 'Float') finalValue = parseFloat(value);

        newRoot.Compound.abilities.Compound[key] = { [type]: finalValue };
        setCurrentData(newRoot);
    };

    const handleVersionChange = (key: string, value: string) => {
        setCurrentData(setVersionStr(currentData, key, value));
    };

    const handleSaveClick = async () => {
        if (isSaving) return;
        let dataToSave = currentData;
        if (mode === 'json') {
            if (editorMarkers.length > 0) {
                toast.error(t("LevelDatEditor.errors.fix_json"));
                return;
            }
            try {
                dataToSave = JSON.parse(jsonString);
            } catch (e) {
                toast.error(t("LevelDatEditor.errors.json_parse_failed"));
                return;
            }
        }
        if (!dataToSave?.Compound) {
            toast.error(t("LevelDatEditor.errors.invalid_data"));
            return;
        }
        setIsSaving(true);
        try { await onSave(dataToSave); } catch (e) { } finally { setIsSaving(false); }
    };

    // --- 渲染组件 ---
    const renderToggle = (label: string, key: string, tip?: string) => {
        const val = getVal(currentData, key, 0);
        const isChecked = Number(val) !== 0;
        return (
            <div className="le-toggle-item" onClick={() => handleChange(key, 'Byte', isChecked ? 0 : 1)} title={tip || key}>
                <div className={`le-toggle-icon ${isChecked ? 'checked' : ''}`}>
                    {isChecked ? <CheckSquare size={16}/> : <Square size={16}/>}
                </div>
                <div className="le-toggle-text">
                    <span className="le-toggle-label">{label}</span>
                </div>
            </div>
        );
    };

    const renderAbilityToggle = (label: string, key: string) => {
        const val = getAbilityVal(currentData, key, 0);
        const isChecked = Number(val) !== 0;
        return (
            <div className="le-toggle-item" onClick={() => handleAbilityChange(key, 'Byte', isChecked ? 0 : 1)} title={`abilities.${key}`}>
                <div className={`le-toggle-icon ${isChecked ? 'checked' : ''}`}>
                    {isChecked ? <CheckSquare size={16}/> : <Square size={16}/>}
                </div>
                <div className="le-toggle-text">
                    <span className="le-toggle-label">{label}</span>
                </div>
            </div>
        );
    };

    const renderAbilityInput = (label: string, key: string, placeholder?: string) => {
        const val = getAbilityVal(currentData, key, 0.05);
        return (
            <div className="le-form-group">
                <label>{label} <span className="le-label-key">({key})</span></label>
                <input
                    className="le-input"
                    type="number"
                    value={val}
                    onChange={(e) => handleAbilityChange(key, 'Float', e.target.value)}
                    placeholder={placeholder}
                    step="0.01"
                />
            </div>
        );
    };

    const renderInput = (label: string, key: string, type: 'String' | 'Int' | 'Long' | 'Float', placeholder?: string) => {
        const val = getVal(currentData, key, '');
        return (
            <div className="le-form-group">
                <label>{label} <span className="le-label-key">({key})</span></label>
                <input
                    className="le-input"
                    type={type === 'String' ? 'text' : 'number'}
                    value={val}
                    onChange={(e) => handleChange(key, type, e.target.value)}
                    placeholder={placeholder}
                    step={type === 'Float' ? '0.1' : '1'}
                />
            </div>
        );
    };

    const renderVersionInput = (label: string, key: string) => {
        const val = getVersionStr(currentData, key);
        return (
            <div className="le-form-group">
                <label>{label} <span className="le-label-key">({key})</span></label>
                <input className="le-input" type="text" value={val} onChange={(e) => handleVersionChange(key, e.target.value)} placeholder="1.21.0.0.0" />
            </div>
        );
    };

    const renderTextarea = (label: string, key: string) => {
        const val = getVal(currentData, key, '');
        return (
            <div className="le-form-group full-width">
                <label>{label} <span className="le-label-key">({key})</span></label>
                <textarea
                    className="le-textarea custom-scrollbar"
                    rows={4}
                    value={val}
                    onChange={(e) => handleChange(key, 'String', e.target.value)}
                    spellCheck={false}
                />
            </div>
        );
    };

    const renderForm = () => {
        const gameType = getVal(currentData, 'GameType', 0);
        const difficulty = getVal(currentData, 'Difficulty', 2);
        const generator = getVal(currentData, 'Generator', 1);
        const spawnX = getVal(currentData, 'SpawnX', 0);
        const spawnY = getVal(currentData, 'SpawnY', 0);
        const spawnZ = getVal(currentData, 'SpawnZ', 0);
        const time = getVal(currentData, 'Time', 0);

        return (
            <div className="le-form-container custom-scrollbar">
                {/* 1. 基础设置 */}
                <div className="le-section" style={{ animationDelay: '0ms' }}>
                    <div className="le-section-header"><Info size={14} /> {t("LevelDatEditor.section_basic")}</div>
                    {renderInput(t("LevelDatEditor.label_map_name"), "LevelName", "String")}
                    <div className="le-grid-3">
                        <div className="le-form-group">
                            <label>{t("LevelDatEditor.label_game_mode")}</label>
                            <Select value={gameType} onChange={(v: any) => handleChange('GameType', 'Int', v)} options={GAME_TYPE_OPTIONS} style={{ width: '100%' }} />
                        </div>
                        <div className="le-form-group">
                            <label>{t("LevelDatEditor.label_difficulty")}</label>
                            <Select value={difficulty} onChange={(v: any) => handleChange('Difficulty', 'Int', v)} options={DIFFICULTY_OPTIONS} style={{ width: '100%' }} />
                        </div>
                        <div className="le-form-group">
                            <label>{t("LevelDatEditor.label_world_type")}</label>
                            <Select value={generator} onChange={(v: any) => handleChange('Generator', 'Int', v)} options={GENERATOR_OPTIONS} style={{ width: '100%' }} />
                        </div>
                    </div>
                    {/* Textarea: 现在可以拖动缩放了 */}
                    {(generator == 2 || getVal(currentData, 'FlatWorldLayers')) && renderTextarea(t("LevelDatEditor.label_flat_layers"), "FlatWorldLayers")}

                    {renderInput(t("LevelDatEditor.label_biome_override"), "BiomeOverride", "String")}
                    <div className="le-grid-2">
                        {renderInput(t("LevelDatEditor.label_world_seed"), "RandomSeed", "Long")}
                        {renderInput(t("LevelDatEditor.label_inventory_version"), "InventoryVersion", "String")}
                    </div>
                    <div className="le-grid-2">
                        {renderVersionInput(t("LevelDatEditor.label_min_version"), "MinimumCompatibleClientVersion")}
                        {renderVersionInput(t("LevelDatEditor.label_last_version"), "lastOpenedWithVersion")}
                    </div>
                </div>

                {/* 2. 游戏规则 */}
                <div className="le-section" style={{ animationDelay: '50ms' }}>
                    <div className="le-section-header"><Settings size={14} /> {t("LevelDatEditor.section_rules")}</div>
                    <div className="le-subsection-title">{t("LevelDatEditor.subsection_core")}</div>
                    <div className="le-toggles-grid">
                        {renderToggle(t("LevelDatEditor.toggle_cheats"), "cheatsEnabled")}
                        {renderToggle(t("LevelDatEditor.toggle_hardcore"), "IsHardcore")}
                        {renderToggle(t("LevelDatEditor.toggle_command_blocks"), "commandblocksenabled")}
                        {renderToggle(t("LevelDatEditor.toggle_command_block_output"), "commandblockoutput")}
                        {renderToggle(t("LevelDatEditor.toggle_admin_commands"), "commandsEnabled")}
                        {renderToggle(t("LevelDatEditor.toggle_command_feedback"), "sendcommandfeedback")}
                        {renderToggle(t("LevelDatEditor.toggle_bonus_chest"), "bonusChestEnabled")}
                        {renderToggle(t("LevelDatEditor.toggle_start_map"), "startWithMapEnabled")}
                        {renderToggle(t("LevelDatEditor.toggle_immediate_respawn"), "doimmediaterespawn")}
                        {renderToggle(t("LevelDatEditor.toggle_recipe_unlock"), "recipesunlock")}
                        {renderToggle(t("LevelDatEditor.toggle_limited_crafting"), "dolimitedcrafting")}
                        {renderToggle(t("LevelDatEditor.toggle_texture_required"), "texturePacksRequired")}
                    </div>

                    <div className="le-subsection-title" style={{ marginTop: 16 }}>{t("LevelDatEditor.subsection_ui")}</div>
                    <div className="le-toggles-grid">
                        {renderToggle(t("LevelDatEditor.toggle_show_coordinates"), "showcoordinates")}
                        {renderToggle(t("LevelDatEditor.toggle_show_days"), "showdaysplayed")}
                        {renderToggle(t("LevelDatEditor.toggle_show_death"), "showdeathmessages")}
                        {renderToggle(t("LevelDatEditor.toggle_show_recipe"), "showrecipemessages")}
                        {renderToggle(t("LevelDatEditor.toggle_show_tags"), "showtags")}
                        {renderToggle(t("LevelDatEditor.toggle_show_border"), "showbordereffect")}
                    </div>

                    <div className="le-subsection-title" style={{ marginTop: 16 }}>{t("LevelDatEditor.subsection_mob_env")}</div>
                    <div className="le-toggles-grid">
                        {renderToggle(t("LevelDatEditor.toggle_daylight_cycle"), "dodaylightcycle")}
                        {renderToggle(t("LevelDatEditor.toggle_weather_cycle"), "doweathercycle")}
                        {renderToggle(t("LevelDatEditor.toggle_mob_spawning"), "domobspawning")}
                        {renderToggle(t("LevelDatEditor.toggle_phantom_spawning"), "doinsomnia")}
                        {renderToggle(t("LevelDatEditor.toggle_mob_griefing"), "mobgriefing")}
                        {renderToggle(t("LevelDatEditor.toggle_mob_loot"), "domobloot")}
                        {renderToggle(t("LevelDatEditor.toggle_entity_drops"), "doentitydrops")}
                        {renderToggle(t("LevelDatEditor.toggle_tile_drops"), "dotiledrops")}
                        {renderToggle(t("LevelDatEditor.toggle_fire_spread"), "dofiretick")}
                        {renderToggle(t("LevelDatEditor.toggle_tnt_explodes"), "tntexplodes")}
                        {renderToggle(t("LevelDatEditor.toggle_respawn_explodes"), "respawnblocksexplode")}
                    </div>
                </div>

                {/* 3. 玩家状态 */}
                <div className="le-section" style={{ animationDelay: '100ms' }}>
                    <div className="le-section-header"><Shield size={14} /> {t("LevelDatEditor.section_player")}</div>
                    <div className="le-toggles-grid">
                        {renderToggle(t("LevelDatEditor.toggle_keep_inventory"), "keepinventory")}
                        {renderToggle(t("LevelDatEditor.toggle_natural_regen"), "naturalregeneration")}
                        {renderToggle(t("LevelDatEditor.toggle_pvp"), "pvp")}
                        {renderToggle(t("LevelDatEditor.toggle_fall_damage"), "falldamage")}
                        {renderToggle(t("LevelDatEditor.toggle_fire_damage"), "firedamage")}
                        {renderToggle(t("LevelDatEditor.toggle_drowning_damage"), "drowningdamage")}
                        {renderToggle(t("LevelDatEditor.toggle_freeze_damage"), "freezedamage")}
                    </div>
                    <div className="le-form-group" style={{ marginTop: 12 }}>
                        <label>{t("LevelDatEditor.label_spawn_point")}</label>
                        <div className="le-xyz-group">
                            <div className="xyz-input"><span>X</span><input className="le-input-ghost" type="number" value={spawnX} onChange={(e) => handleChange('SpawnX', 'Int', e.target.value)} /></div>
                            <div className="xyz-input"><span>Y</span><input className="le-input-ghost" type="number" value={spawnY} onChange={(e) => handleChange('SpawnY', 'Int', e.target.value)} /></div>
                            <div className="xyz-input"><span>Z</span><input className="le-input-ghost" type="number" value={spawnZ} onChange={(e) => handleChange('SpawnZ', 'Int', e.target.value)} /></div>
                        </div>
                    </div>
                </div>

                {/* 4. 玩家能力 (Abilities) */}
                <div className="le-section" style={{ animationDelay: '125ms' }}>
                    <div className="le-section-header"><Zap size={14} /> {t("LevelDatEditor.section_abilities")}</div>
                    <div className="le-subsection-title">{t("LevelDatEditor.subsection_permissions")}</div>
                    <div className="le-toggles-grid">
                        {renderAbilityToggle(t("LevelDatEditor.ability_mine"), "mine")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_build"), "build")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_attack_mobs"), "attackmobs")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_attack_players"), "attackplayers")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_doors"), "doorsandswitches")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_open_containers"), "opencontainers")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_op"), "op")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_teleport"), "teleport")}
                    </div>

                    <div className="le-subsection-title" style={{ marginTop: 16 }}>{t("LevelDatEditor.subsection_movement")}</div>
                    <div className="le-toggles-grid">
                        {renderAbilityToggle(t("LevelDatEditor.ability_flying"), "flying")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_mayfly"), "mayfly")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_instabuild"), "instabuild")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_invulnerable"), "invulnerable")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_noclip"), "noclip")}
                        {renderAbilityToggle(t("LevelDatEditor.ability_lightning"), "lightning")}
                    </div>

                    <div className="le-grid-3" style={{ marginTop: 12 }}>
                        {renderAbilityInput(t("LevelDatEditor.ability_walk_speed"), "walkSpeed")}
                        {renderAbilityInput(t("LevelDatEditor.ability_fly_speed"), "flySpeed")}
                        {renderAbilityInput(t("LevelDatEditor.ability_vertical_speed"), "verticalFlySpeed")}
                    </div>
                </div>

                {/* 5. 环境设置 */}
                <div className="le-section" style={{ animationDelay: '150ms' }}>
                    <div className="le-section-header"><Clock size={14} /> {t("LevelDatEditor.section_environment")}</div>
                    <div className="le-form-group">
                        <label>{t("LevelDatEditor.label_time")}</label>
                        {/* 复合输入组：现在布局修正了 */}
                        <div className="le-input-group">
                            <input className="le-input" type="number" value={time} onChange={(e) => handleChange('Time', 'Long', e.target.value)} />
                            <div className="le-btn-group-sm">
                                <button className="le-btn-mini" onClick={() => handleChange('Time', 'Long', 0)}>{t("LevelDatEditor.time_morning")}</button>
                                <button className="le-btn-mini" onClick={() => handleChange('Time', 'Long', 6000)}>{t("LevelDatEditor.time_noon")}</button>
                                <button className="le-btn-mini" onClick={() => handleChange('Time', 'Long', 13000)}>{t("LevelDatEditor.time_midnight")}</button>
                            </div>
                        </div>
                    </div>
                    <div className="le-grid-2">
                        {renderInput(t("LevelDatEditor.label_current_tick"), "currentTick", "Long")}
                        {renderInput(t("LevelDatEditor.label_random_tick"), "randomtickspeed", "Int")}
                    </div>
                    <div className="le-grid-2">
                        {renderInput(t("LevelDatEditor.label_rain_level"), "rainLevel", "Float")}
                        {renderInput(t("LevelDatEditor.label_rain_time"), "rainTime", "Int")}
                    </div>
                    <div className="le-grid-2">
                        {renderInput(t("LevelDatEditor.label_lightning_level"), "lightningLevel", "Float")}
                        {renderInput(t("LevelDatEditor.label_lightning_time"), "lightningTime", "Int")}
                    </div>
                </div>

                {/* 6. 高级与网络 */}
                <div className="le-section" style={{ animationDelay: '200ms' }}>
                    <div className="le-section-header"><Globe size={14} /> {t("LevelDatEditor.section_advanced")}</div>
                    <div className="le-toggles-grid">
                        {renderToggle(t("LevelDatEditor.toggle_multiplayer"), "MultiplayerGame")}
                        {renderToggle(t("LevelDatEditor.toggle_lan"), "LANBroadcast")}
                        {renderToggle(t("LevelDatEditor.toggle_education_features"), "educationFeaturesEnabled")}
                        {renderToggle(t("LevelDatEditor.toggle_allow_destructive"), "allowdestructiveobjects")}
                        {renderToggle(t("LevelDatEditor.toggle_global_mute"), "globalmute")}
                    </div>
                    <div className="le-grid-2" style={{ marginTop: 12 }}>
                        {renderInput(t("LevelDatEditor.label_tick_range"), "serverChunkTickRange", "Int")}
                        {renderInput(t("LevelDatEditor.label_nether_scale"), "NetherScale", "Int")}
                    </div>
                    <div className="le-grid-2">
                        {renderInput(t("LevelDatEditor.label_sleep_percentage"), "playerssleepingpercentage", "Int")}
                        {renderInput(t("LevelDatEditor.label_spawn_radius"), "spawnradius", "Int")}
                    </div>
                    <div className="le-grid-2">
                        {renderInput(t("LevelDatEditor.label_network_version"), "NetworkVersion", "Int")}
                        {renderInput(t("LevelDatEditor.label_platform"), "Platform", "Int")}
                    </div>
                </div>

                {/* 底部留白 */}
                <div style={{ height: 40, flexShrink: 0 }}></div>
            </div>
        );
    };

    return (
        <div className="level-editor-wrapper">
            <div className="le-header">
                <div className="le-header-left">
                    <button className="le-back-btn" onClick={onBack} title={t("common.back")}><ArrowLeft size={18} /></button>
                    <div className="le-title-group">
                        <h3>{t("LevelDatEditor.title")}</h3>
                        <span className="le-subtitle">{fileName}</span>
                    </div>
                </div>
                <div className="le-header-actions">
                    {/* [新增] 启动按钮 (只在有 onLaunch 回调时显示) */}
                    {onLaunch && (
                        <button className="le-btn-launch" onClick={onLaunch} title={t("LevelDatEditor.launch_title")}>
                            <Play size={14} fill="currentColor" /> {t("LevelDatEditor.launch")}
                        </button>
                    )}

                    <div className="le-mode-switch">
                        <button className={`le-mode-btn ${mode === 'form' ? 'active' : ''}`} onClick={() => handleModeSwitch('form')}>
                            <LayoutTemplate size={14} /> {t("LevelDatEditor.mode_form")}
                        </button>
                        <button className={`le-mode-btn ${mode === 'json' ? 'active' : ''}`} onClick={() => handleModeSwitch('json')}>
                            <FileJson size={14} /> {t("LevelDatEditor.mode_json")}
                        </button>
                    </div>

                    <Button size="sm" variant="primary" onClick={handleSaveClick} disabled={isSaving || (mode === 'json' && editorMarkers.length > 0)} style={{ marginLeft: 12, height: 28 }}>
                        {isSaving ? <RefreshCcw size={14} className="spin" /> : <Save size={14} />}
                        {isSaving ? ` ${t("common.saving")}` : ` ${t("common.save")}`}
                    </Button>
                </div>
            </div>

            <div className="le-body">
                {mode === 'form' ? renderForm() : (
                    <div className="le-json-container anim-fade-in">
                        <CodeMirror
                            value={jsonString}
                            height="100%"
                            theme={vscodeDark}
                            extensions={[json()]}
                            onChange={handleCodeMirrorChange}
                            basicSetup={{
                                lineNumbers: true,
                                foldGutter: true,
                                highlightActiveLine: true,
                                autocompletion: true,
                                lintKeymap: true,
                            }}
                            style={{
                                fontSize: '13px',
                                height: '100%',
                                fontFamily: "'JetBrains Mono', 'Fira Code', Consolas, monospace"
                            }}
                        />
                        {editorMarkers.length > 0 && (
                            <div className="le-json-error-banner">
                                <AlertCircle size={14} />
                                <span>{t("LevelDatEditor.json_error_banner")}</span>
                            </div>
                        )}
                    </div>
                )}
            </div>
        </div>
    );
};
