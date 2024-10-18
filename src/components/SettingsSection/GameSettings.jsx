import React from "react";

function GameSettings({ injectDll, setInjectDll, injectorPath, setInjectorPath, launcherVisibility, setLauncherVisibility }) {
    return (
        <div className="section">
            <div className="toggle-container">
                <span>启动游戏时是否注入对应 DLL</span>
                <label className="switch">
                    <input
                        type="checkbox"
                        checked={injectDll}
                        onChange={() => setInjectDll(!injectDll)}
                    />
                    <span className="slider round"></span>
                </label>
            </div>
            <div className="custom-injector">
                <label htmlFor="injectorPath">自定义注入器路径:</label>
                <input
                    type="text"
                    id="injectorPath"
                    value={injectorPath}
                    onChange={(e) => setInjectorPath(e.target.value)}
                    placeholder="选择自定义 DLL 注入器路径"
                />
            </div>

            <div className="launcher-visibility">
                <label htmlFor="visibility">启动器可见性:</label>
                <select
                    id="visibility"
                    value={launcherVisibility}
                    onChange={(e) => setLauncherVisibility(e.target.value)}
                >
                    <option value="立即关闭">游戏启动后立即关闭</option>
                    <option value="最小化">游戏启动最小化</option>
                    <option value="保持不变">游戏启动后保持不变</option>
                </select>
            </div>
        </div>
    );
}

export default GameSettings;
